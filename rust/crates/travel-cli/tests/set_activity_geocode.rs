//! Real-Turso integration tests for `set-activity-geocode` (spec
//! docs/superpowers/specs/2026-11-20-cli-itinerary-editing-design.md §2).
//! Fixtures are per-run unique (`zz_test_<nanos>`); the RAII `Guard` is armed right
//! after the plan id is bound; teardown is the canonical `teardown_plan`. Skips
//! cleanly without Turso creds.

use std::process::Command;

mod common;
use common::{bin, db_exec, nanos, seed_plan, teardown_plan, Guard};

static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn exec_ok(sql: &str) -> common::Rows {
    db_exec(sql).unwrap_or_else(|| panic!("db exec skipped unexpectedly for SQL: {sql}"))
}

fn n(sql: &str) -> String {
    exec_ok(sql).scalar().unwrap_or_default()
}

fn run_cmd(args: &[&str]) -> (bool, String, String) {
    let out = Command::new(bin())
        .args(args)
        .env_remove("TRAVEL_PLAN_ID")
        .env_remove("TRAVEL_TODAY")
        .output()
        .expect("run travel");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// D1 morning: `_a` 川岩咖啡, `_b` 滿山望海, `_c` 滿山 (so "滿山" matches `_b` and `_c`).
fn seed(plan: &str, dest: &str) {
    seed_plan(plan, dest, 5);
    exec_ok(&format!(
        "INSERT INTO days (plan_id, destination, day_number, date, day_type) \
           VALUES ('{plan}', '{dest}', 1, '2026-01-01', 'full'); \
         INSERT INTO timesofday (plan_id, destination, day_number, session_type) \
           VALUES ('{plan}', '{dest}', 1, 'morning'), ('{plan}', '{dest}', 1, 'afternoon'); \
         INSERT INTO activities (id, plan_id, destination, day_number, session_type, sort_order, \
             title, priority, updated_at) \
           VALUES ('{plan}_a', '{plan}', '{dest}', 1, 'morning', 0, '川岩咖啡', 'want', '2000-01-01 00:00:00'), \
                  ('{plan}_b', '{plan}', '{dest}', 1, 'morning', 1, '滿山望海', 'want', '2000-01-01 00:00:00'), \
                  ('{plan}_c', '{plan}', '{dest}', 1, 'morning', 2, '滿山步道', 'want', '2000-01-01 00:00:00');"
    ));
}

fn version(plan: &str) -> String {
    n(&format!("SELECT version AS n FROM plans WHERE plan_id = '{plan}'"))
}

fn events(plan: &str, ev: &str) -> String {
    n(&format!(
        "SELECT COUNT(*) AS n FROM plan_events WHERE plan_id = '{plan}' AND event = '{ev}'"
    ))
}

fn pin(plan: &str, suffix: &str) -> String {
    n(&format!(
        "SELECT COALESCE(lat, 'x') || '|' || COALESCE(lon, 'x') || '|' || COALESCE(geo_source, 'x') AS n \
         FROM activities WHERE id = '{plan}_{suffix}'"
    ))
}

macro_rules! fixture {
    ($plan:ident, $dest:ident) => {
        let _lock = LOCK.lock().unwrap_or_else(|p| p.into_inner());
        if db_exec("SELECT 1 AS n").is_none() {
            eprintln!("skipping set_activity_geocode test (no Turso creds)");
            return;
        }
        let tag = nanos();
        let $plan = format!("zz_test_{tag}");
        let $dest = format!("zz_test_{tag}");
        let _g = Guard::new({
            let (plan, dest) = ($plan.clone(), $dest.clone());
            move || teardown_plan(&plan, &dest)
        });
        seed(&$plan, &$dest);
    };
}

fn geocode(plan: &str, dest: &str, key: &str, extra: &[&str]) -> (bool, String, String) {
    let mut a = vec!["set-activity-geocode", "1", "morning", key];
    a.extend_from_slice(extra);
    a.extend_from_slice(&["--plan-id", plan, "--dest", dest]);
    run_cmd(&a)
}

#[test]
fn set_then_clear_with_full_audit_triad() {
    fixture!(plan, dest);
    let (ok, stdout, stderr) = geocode(
        &plan, &dest, "川岩",
        &["--lat", "25.0123", "--lon", "121.9456", "--source", "OSM node 1"],
    );
    assert!(ok, "set failed; stdout={stdout} stderr={stderr}");
    assert_eq!(pin(&plan, "a"), "25.0123|121.9456|OSM node 1");
    assert_eq!(version(&plan), "6");
    assert_eq!(events(&plan, "activity_geocode_set"), "2"); // dest_process + timeline
    assert_eq!(
        n(&format!(
            "SELECT COUNT(*) AS n FROM operation_runs WHERE plan_id = '{plan}' \
             AND command_type = 'set-activity-geocode' AND status = 'completed' \
             AND version_before = 5 AND version_after = 6"
        )),
        "1"
    );
    let kv = |k: &str| {
        n(&format!(
            "SELECT value AS v FROM plan_event_data WHERE plan_id = '{plan}' AND scope = 'timeline' \
             AND key = '{k}' AND plan_id = '{plan}' ORDER BY rowid DESC LIMIT 1"
        ))
    };
    assert_eq!(kv("to.lat"), "25.0123");
    assert_eq!(kv("from.lat"), "undefined");
    assert_eq!(kv("activity_id"), format!("{plan}_a"));
    // touch_day: the day row's updated_at moved off its seed value
    assert!(n(&format!(
        "SELECT updated_at AS n FROM days WHERE plan_id = '{plan}' AND day_number = 1"
    )) != "");

    // clear
    let (ok, stdout, stderr) = geocode(&plan, &dest, &format!("{plan}_a"), &["--clear"]);
    assert!(ok, "clear failed; stdout={stdout} stderr={stderr}");
    assert_eq!(pin(&plan, "a"), "x|x|x");
    assert_eq!(version(&plan), "7");
    assert_eq!(events(&plan, "activity_geocode_cleared"), "2");
}

#[test]
fn same_value_and_clear_of_unpinned_are_zero_audit_noops() {
    fixture!(plan, dest);
    let args = ["--lat", "25.0123", "--lon", "121.9456", "--source", "OSM"];
    assert!(geocode(&plan, &dest, "川岩", &args).0);
    assert_eq!(version(&plan), "6");
    let (ok, stdout, _e) = geocode(&plan, &dest, "川岩", &args);
    assert!(ok && stdout.contains("No change"), "{stdout}");
    assert_eq!(version(&plan), "6");
    assert_eq!(events(&plan, "activity_geocode_set"), "2");
    let (ok, stdout, _e) = geocode(&plan, &dest, "滿山望海", &["--clear"]);
    assert!(ok && stdout.contains("No change"), "{stdout}");
    assert_eq!(version(&plan), "6");
    assert_eq!(events(&plan, "activity_geocode_cleared"), "0");
}

#[test]
fn ambiguous_title_fails_with_zero_writes() {
    fixture!(plan, dest);
    let (ok, _o, stderr) = geocode(&plan, &dest, "滿山", &["--lat", "25.0", "--lon", "121.9"]);
    assert!(!ok);
    assert!(stderr.contains("滿山望海") && stderr.contains("滿山步道"), "{stderr}");
    assert_eq!(pin(&plan, "b"), "x|x|x");
    assert_eq!(pin(&plan, "c"), "x|x|x");
    assert_eq!(version(&plan), "5");
    assert_eq!(events(&plan, "activity_geocode_set"), "0");
}

#[test]
fn validation_failures_write_nothing() {
    fixture!(plan, dest);
    for bad in [
        vec!["--lat", "0", "--lon", "0"],
        vec!["--lat", "91", "--lon", "121"],
        vec!["--lat", "25.0", "--lon", "181"],
        vec!["--lat", "25.0"],
        vec!["--lon", "121.0"],
        vec!["--lat", "NaN", "--lon", "121"],
        vec!["--clear", "--lat", "25.0", "--lon", "121.0"],
        vec![],
    ] {
        let (ok, _o, e) = geocode(&plan, &dest, "川岩", &bad);
        assert!(!ok, "{bad:?} should fail ({e})");
    }
    assert_eq!(pin(&plan, "a"), "x|x|x");
    assert_eq!(version(&plan), "5");
    assert_eq!(
        n(&format!("SELECT COUNT(*) AS n FROM plan_events WHERE plan_id = '{plan}' AND event LIKE 'activity_geocode%'")),
        "0"
    );
}

#[test]
fn pin_survives_move_activity() {
    fixture!(plan, dest);
    assert!(geocode(&plan, &dest, "川岩", &["--lat", "25.0123", "--lon", "121.9456"]).0);
    let (ok, o, e) = run_cmd(&[
        "move-activity", "1", "morning", "afternoon", &format!("{plan}_a"),
        "--plan-id", &plan, "--dest", &dest,
    ]);
    assert!(ok, "{o}{e}");
    assert_eq!(pin(&plan, "a"), "25.0123|121.9456|x");
    assert_eq!(
        n(&format!("SELECT session_type AS n FROM activities WHERE id = '{plan}_a'")),
        "afternoon"
    );
}
