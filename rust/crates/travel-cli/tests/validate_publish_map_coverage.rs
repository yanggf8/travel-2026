//! Behavior locks for per-day map coverage warnings in `travel validate publish`.
//!
//! These tests hit shared Turso through the canonical `common::` harness. They
//! must be run serialized and in the background:
//! `cargo test -p travel-cli --test validate_publish_map_coverage -- --test-threads=1`.

mod common;
use common::{
    bin, db_exec, db_exec_teardown, is_credless, nanos, seed_plan, teardown_plan, Guard,
};

use std::process::Command;

const TODAY: &str = "2026-07-05";

fn run_publish(plan_id: &str, dest: &str) -> (bool, String, String) {
    let out = Command::new(bin())
        .args(["validate", "publish", "--plan-id", plan_id, "--dest", dest])
        .env("TRAVEL_TODAY", TODAY)
        .env_remove("TRAVEL_PLAN_ID")
        .output()
        .unwrap_or_else(|e| panic!("run travel validate publish: {e}"));
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn seed_destination(dest: &str) {
    db_exec(&format!(
        "INSERT OR IGNORE INTO destination_config \
           (slug, display_name, ref_id, ref_path, timezone, currency, language, origin) \
         VALUES ('{dest}', 'ZZ Map Coverage Test', 'zz-ref', 'turso:destination-ref/zz', \
                 'Asia/Tokyo', 'JPY', 'ja', 'taiwan');"
    ))
    .expect("seed destination_config");
}

fn teardown_destination(dest: &str) {
    let _ = db_exec_teardown(&format!(
        "DELETE FROM destination_pois WHERE slug = '{dest}'; \
         DELETE FROM destination_config WHERE slug = '{dest}';"
    ));
}

fn seed_anchor(plan_id: &str, dest: &str, start: &str, end: &str, days: i64) {
    db_exec(&format!(
        "INSERT INTO date_anchors (plan_id, destination, start_date, end_date, days) \
         VALUES ('{plan_id}', '{dest}', '{start}', '{end}', {days});"
    ))
    .expect("seed date_anchors");
}

#[test]
fn null_poi_activity_day_warns_but_linked_and_zero_activity_days_do_not() {
    let tag = nanos();
    let plan_id = format!("test-mapcov-warn-{tag}");
    let dest = format!("mapcovwarn_{tag}");
    let _g = Guard::new({
        let (plan_id, dest) = (plan_id.clone(), dest.clone());
        move || {
            teardown_destination(&dest);
            teardown_plan(&plan_id, &dest);
        }
    });

    if db_exec("SELECT 1").is_none() {
        return;
    }

    seed_plan(&plan_id, &dest, 0);
    seed_destination(&dest);
    seed_anchor(&plan_id, &dest, "2026-02-01", "2026-02-03", 3);

    db_exec(&format!(
        "INSERT INTO destination_pois \
           (slug, poi_id, title, source_url, fetched_at, confidence, lat, lon) \
         VALUES ('{dest}', 'mapped_poi', 'Mapped POI', 'test', '2026-07-05', 'test', \
                 35.6812, 139.7671); \
         INSERT INTO days (plan_id, destination, day_number, date, day_type, theme, theme_zh) \
         VALUES ('{plan_id}', '{dest}', 1, '2026-02-01', 'arrival', 'Mapped', '有地圖'), \
                ('{plan_id}', '{dest}', 2, '2026-02-02', 'full', 'Unmapped', '無地圖'), \
                ('{plan_id}', '{dest}', 3, '2026-02-03', 'departure', 'Empty', '空白'); \
         INSERT INTO activities \
           (id, plan_id, destination, poi_id, day_number, session_type, sort_order, title, \
            booking_required, is_fixed_time, priority, source) \
         VALUES ('{plan_id}-mapped', '{plan_id}', '{dest}', 'mapped_poi', 1, 'morning', 0, \
                 'Mapped activity', 0, 0, 'want', 'confirmed'), \
                ('{plan_id}-unmapped', '{plan_id}', '{dest}', NULL, 2, 'morning', 0, \
                 'Unmapped real place', 0, 0, 'want', 'confirmed'); \
         INSERT INTO plan_map_snapshots (plan_id, snapshotted_at) \
         VALUES ('{plan_id}', datetime('now'));"
    ))
    .expect("seed map coverage case");

    let (_ok, stdout, stderr) = run_publish(&plan_id, &dest);
    if is_credless(&stderr) {
        return;
    }

    assert!(
        stdout.contains("[map-coverage] day 2 has no geocoded stops and no route segments"),
        "day 2 has an activity with poi_id=NULL and must warn. stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        !stdout.contains("[map-coverage] day 1 "),
        "day 1 has a linked geocoded POI and must not warn. stdout:\n{stdout}"
    );
    assert!(
        !stdout.contains("[map-coverage] day 3 "),
        "day 3 has zero activities and must not warn. stdout:\n{stdout}"
    );
}

#[test]
fn route_segment_day_does_not_warn_even_without_geocoded_activity_stops() {
    let tag = nanos();
    let plan_id = format!("test-mapcov-route-{tag}");
    let dest = format!("mapcovroute_{tag}");
    let _g = Guard::new({
        let (plan_id, dest) = (plan_id.clone(), dest.clone());
        move || {
            teardown_destination(&dest);
            teardown_plan(&plan_id, &dest);
        }
    });

    if db_exec("SELECT 1").is_none() {
        return;
    }

    seed_plan(&plan_id, &dest, 0);
    seed_destination(&dest);
    seed_anchor(&plan_id, &dest, "2026-02-01", "2026-02-01", 1);

    db_exec(&format!(
        "INSERT INTO days (plan_id, destination, day_number, date, day_type, theme, theme_zh) \
         VALUES ('{plan_id}', '{dest}', 2, '2026-02-01', 'full', 'Route day', '路線日'); \
         INSERT INTO activities \
           (id, plan_id, destination, poi_id, day_number, session_type, sort_order, title, \
            booking_required, is_fixed_time, priority, source) \
         VALUES ('{plan_id}-unmapped', '{plan_id}', '{dest}', NULL, 2, 'morning', 0, \
                 'Unmapped real place', 0, 0, 'want', 'confirmed'); \
         INSERT INTO day_route_segments \
           (plan_id, destination, day_number, sort_order, from_place, to_place, mode, source) \
         VALUES ('{plan_id}', '{dest}', 2, 0, 'Station A', 'Station B', 'train', 'confirmed'); \
         INSERT INTO plan_map_snapshots (plan_id, snapshotted_at) \
         VALUES ('{plan_id}', datetime('now'));"
    ))
    .expect("seed route coverage case");

    let (_ok, stdout, stderr) = run_publish(&plan_id, &dest);
    if is_credless(&stderr) {
        return;
    }

    assert!(
        !stdout.contains("[map-coverage]"),
        "a day with route segments already has a dashboard route path and must not warn. stdout:\n{stdout}"
    );
}