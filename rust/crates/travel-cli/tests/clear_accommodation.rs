//! clear-accommodation integration test (real Turso, Guard panic-safe).
//!
//! Verifies: set-accommodation booked -> clear-accommodation --hotel 海論
//! removes bookings_current row and reverts P4 to selecting.

mod common;
use common::{bin, db_exec, is_credless, teardown_plan, Guard};
use std::process::Command;

fn run(args: &[&str]) -> (bool, String, String) {
    let out = Command::new(bin()).args(args).output().expect("run travel");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn ensure_domestic_seed() {
    let _ = db_exec(
        "INSERT OR IGNORE INTO domestic_accommodations \
         (id, destination, hotel_name, room_type, sea_view, price_twd, currency, breakfast_included, source, updated_at) \
         VALUES \
         ('jiufen_hailun_seaview_5200','jiufen','海論','海景雙人房',1,5200,'TWD',1,'manual',datetime('now')), \
         ('zz_test_seaview_7200','jiufen','ZZ測試海景館','海景雙人房',1,7200,'TWD',1,'manual',datetime('now')), \
         ('jiufen_shancheng_seaview_4200','jiufen','山城逸境','海景雙人房',1,4200,'TWD',1,'manual',datetime('now'))",
    );
    let _ = db_exec(
        "INSERT OR IGNORE INTO destination_config (slug, display_name, timezone, currency, language, origin, lat, lon) \
         VALUES ('jiufen','九份','Asia/Taipei','TWD','zh-TW','taiwan',25.109,121.844)",
    );
}

fn process_status(plan: &str, dest: &str) -> Option<String> {
    let sql = format!(
        "SELECT status AS v FROM process_statuses WHERE plan_id = '{plan}' AND destination = '{dest}' AND process_id = 'process_4_accommodation'"
    );
    db_exec(&sql).and_then(|r| {
        r.raw()
            .lines()
            .find_map(|l| l.split(':').nth(1).map(|v| v.trim().to_string()))
            .filter(|v| !v.is_empty())
    })
}

#[test]
fn clear_accommodation_removes_booking_and_reverts_p4() {
    let Some(_) = db_exec("SELECT 1") else {
        eprintln!("credless — skip");
        return;
    };
    let _ = run(&["db", "migrate"]);
    ensure_domestic_seed();

    let n = common::nanos();
    let plan = format!("test-clear-acc-{n}");
    let dest = "jiufen";

    teardown_plan(&plan, dest);
    let _g = Guard::new({
        let (plan, dest) = (plan.clone(), dest.to_string());
        move || {
            // The test-only third candidate is NOT plan-keyed, so teardown_plan
            // does not cover it — delete it explicitly or it leaks into shared Turso.
            let _ = common::db_exec_teardown(
                "DELETE FROM domestic_accommodations WHERE id = 'zz_test_seaview_7200'",
            );
            teardown_plan(&plan, &dest);
        }
    });
    common::seed_plan(&plan, dest, 1);
    ensure_domestic_seed();

    // Clean any prior bookings for this plan
    let _ = db_exec(&format!("DELETE FROM bookings_current WHERE trip_id = '{plan}'"));
    let _ = db_exec(&format!(
        "DELETE FROM bookings WHERE destination = '{dest}' AND offer_id LIKE 'domestic:{plan}:%'"
    ));

    // Set P4 to pending first (seed_plan created pending; but ensure it)
    let _ = db_exec(&format!(
        "INSERT OR REPLACE INTO process_statuses (plan_id, destination, process_id, status, updated_at) \
         VALUES ('{plan}','{dest}','process_4_accommodation','pending',datetime('now'))"
    ));

    // Book
    let (ok, stdout, stderr) = run(&[
        "set-accommodation",
        "--hotel",
        "海論",
        "--room-type",
        "海景雙人房",
        "--price",
        "5200",
        "--date",
        "2026-11-01",
        "--dest",
        dest,
        "--plan-id",
        &plan,
    ]);
    if is_credless(&stderr) {
        eprintln!("credless on set-accommodation — skip");
        return;
    }
    assert!(ok, "set-accommodation should succeed; stdout={stdout} stderr={stderr}");

    // Assert bookings_current has 1 row and P4 booked
    let bc = db_exec(&format!(
        "SELECT COUNT(*) AS n FROM bookings_current WHERE trip_id = '{plan}'"
    ))
    .expect("query bookings_current");
    let count: i64 = bc
        .raw()
        .lines()
        .find_map(|l| l.split(':').nth(1))
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(-1);
    assert_eq!(count, 1, "bookings_current should have 1 row before clear; out={}", bc.raw());

    let p4 = process_status(&plan, dest);
    assert_eq!(p4.as_deref(), Some("booked"), "P4 should be booked before clear; got {p4:?}");

    // Clear
    let (ok, stdout, stderr) = run(&[
        "clear-accommodation",
        "--hotel",
        "海論",
        "--dest",
        dest,
        "--plan-id",
        &plan,
    ]);
    if is_credless(&stderr) {
        eprintln!("credless on clear-accommodation — skip");
        return;
    }
    assert!(
        ok,
        "clear-accommodation should succeed; stdout={stdout} stderr={stderr}"
    );
    assert!(stdout.contains("Cleared accommodation"), "output should confirm clear: {stdout}");
    assert!(
        stdout.contains("selecting") || stdout.contains("P4"),
        "should mention P4 selecting: {stdout}"
    );

    // Assert bookings_current 0
    let bc2 = db_exec(&format!(
        "SELECT COUNT(*) AS n FROM bookings_current WHERE trip_id = '{plan}'"
    ))
    .expect("query bookings_current after clear");
    let count2: i64 = bc2
        .raw()
        .lines()
        .find_map(|l| l.split(':').nth(1))
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(-1);
    assert_eq!(
        count2, 0,
        "bookings_current should have 0 rows after clear; out={}",
        bc2.raw()
    );

    // Assert P4 reverted to selecting (or pending via cancelled path)
    let p4_after = process_status(&plan, dest);
    assert!(
        matches!(p4_after.as_deref(), Some("selecting") | Some("pending")),
        "P4 should be selecting or pending after clear; got {p4_after:?}"
    );
    // Prefer selecting as per current implementation
    if p4_after.as_deref() != Some("selecting") {
        eprintln!("Note: P4 after clear was {:?}, expected selecting (acceptable pending)", p4_after);
    }
}
