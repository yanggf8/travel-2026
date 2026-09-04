//! Domestic flow-decision defer auto-skip integration tests (real Turso, Guard panic-safe).
//!
//! Verifies: `travel flow-decision shop mode --mode defer` for a domestic
//! destination auto-advances P3/P34 to skipped, and conditionally P4:
//!   - when P4 is pending/selecting -> skipped
//!   - when P4 is booked -> preserved (not clobbered)

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
        "INSERT OR IGNORE INTO destination_config (slug, display_name, timezone, currency, language, origin, lat, lon) \
         VALUES ('jiufen','九份','Asia/Taipei','TWD','zh-TW','taiwan',25.109,121.844)",
    );
    let _ = db_exec(
        "UPDATE destination_config SET lat = 25.109, lon = 121.844 \
         WHERE slug = 'jiufen' AND (lat IS NULL OR lon IS NULL)",
    );
    let _ = db_exec(
        "INSERT OR IGNORE INTO domestic_accommodations \
         (id, destination, hotel_name, room_type, sea_view, price_twd, currency, breakfast_included, source, updated_at) \
         VALUES \
         ('jiufen_hailun_seaview_5200','jiufen','海論','海景雙人房',1,5200,'TWD',1,'manual',datetime('now')), \
         ('zz_test_seaview_7200','jiufen','ZZ測試海景館','海景雙人房',1,7200,'TWD',1,'manual',datetime('now')), \
         ('jiufen_shancheng_seaview_4200','jiufen','山城逸境','海景雙人房',1,4200,'TWD',1,'manual',datetime('now'))",
    );
}

fn process_status(plan: &str, dest: &str, pid: &str) -> Option<String> {
    let sql = format!(
        "SELECT status AS v FROM process_statuses WHERE plan_id = '{plan}' AND destination = '{dest}' AND process_id = '{pid}'"
    );
    db_exec(&sql).and_then(|r| {
        r.raw()
            .lines()
            .find_map(|l| l.split(':').nth(1).map(|v| v.trim().to_string()))
            .filter(|v| !v.is_empty())
    })
}

#[test]
fn domestic_defer_auto_advances_p3_p34_p4_to_skipped() {
    let Some(_) = db_exec("SELECT 1") else {
        eprintln!("credless — skip");
        return;
    };
    let _ = run(&["db", "migrate"]);
    ensure_domestic_seed();

    let n = common::nanos();
    let plan = format!("test-domestic-defer-{n}");
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
    ensure_domestic_seed();

    // create-plan with jiufen, 2026-11-01 -> 2026-11-02, airport TPE
    let (ok, stdout, stderr) = run(&[
        "create-plan",
        &plan,
        "--dest",
        dest,
        "--start",
        "2026-11-01",
        "--end",
        "2026-11-02",
        "--airport",
        "TPE",
    ]);
    if is_credless(&stderr) {
        eprintln!("credless on create-plan — skip");
        return;
    }
    assert!(ok, "create-plan should succeed; stdout={stdout} stderr={stderr}");

    // Assert P3/P34/P4 initial pending
    for pid in ["process_3_transportation", "process_3_4_packages", "process_4_accommodation"] {
        let st = process_status(&plan, dest, pid);
        assert_eq!(
            st.as_deref(),
            Some("pending"),
            "initial {pid} should be pending; got {st:?}"
        );
    }

    // flow-decision shop mode --mode defer --reason test
    let (ok, stdout, stderr) = run(&[
        "flow-decision",
        "shop",
        "mode",
        "--mode",
        "defer",
        "--reason",
        "test",
        "--plan-id",
        &plan,
    ]);
    if is_credless(&stderr) {
        eprintln!("credless on flow-decision — skip");
        return;
    }
    assert!(ok, "flow-decision defer should succeed; stdout={stdout} stderr={stderr}");
    assert!(
        stdout.contains("domestic shop deferred") || stdout.contains("P3/P4 marked skipped"),
        "hint present: {stdout}"
    );

    // Assert P3/P34/P4 all skipped via SELECT status
    for pid in ["process_3_transportation", "process_3_4_packages", "process_4_accommodation"] {
        let st = process_status(&plan, dest, pid);
        assert_eq!(
            st.as_deref(),
            Some("skipped"),
            "{pid} should be skipped after defer; got {st:?}"
        );
    }
}

#[test]
fn domestic_defer_preserves_booked_p4() {
    let Some(_) = db_exec("SELECT 1") else {
        eprintln!("credless — skip");
        return;
    };
    let _ = run(&["db", "migrate"]);
    ensure_domestic_seed();

    let n = common::nanos();
    let plan = format!("test-domestic-defer-booked-{n}");
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
    ensure_domestic_seed();

    let (ok, stdout, stderr) = run(&[
        "create-plan",
        &plan,
        "--dest",
        dest,
        "--start",
        "2026-11-01",
        "--end",
        "2026-11-02",
        "--airport",
        "TPE",
    ]);
    if is_credless(&stderr) {
        eprintln!("credless on create-plan — skip");
        return;
    }
    assert!(ok, "create-plan should succeed; stdout={stdout} stderr={stderr}");

    // Book accommodation first: set-accommodation 海論
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
    assert!(
        ok,
        "set-accommodation should succeed; stdout={stdout} stderr={stderr}"
    );

    let p4_before = process_status(&plan, dest, "process_4_accommodation");
    assert_eq!(
        p4_before.as_deref(),
        Some("booked"),
        "P4 should be booked before defer; got {p4_before:?}"
    );

    // Now defer
    let (ok, stdout, stderr) = run(&[
        "flow-decision",
        "shop",
        "mode",
        "--mode",
        "defer",
        "--reason",
        "test-booked",
        "--plan-id",
        &plan,
    ]);
    if is_credless(&stderr) {
        eprintln!("credless on flow-decision — skip");
        return;
    }
    assert!(ok, "flow-decision defer should succeed; stdout={stdout} stderr={stderr}");

    // P3/P34 should be skipped, P4 should remain booked
    for pid in ["process_3_transportation", "process_3_4_packages"] {
        let st = process_status(&plan, dest, pid);
        assert_eq!(
            st.as_deref(),
            Some("skipped"),
            "{pid} should be skipped; got {st:?}"
        );
    }
    let p4_after = process_status(&plan, dest, "process_4_accommodation");
    assert_eq!(
        p4_after.as_deref(),
        Some("booked"),
        "P4 should remain booked after defer; got {p4_after:?}"
    );

    // also ensure booking still exists
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
    assert_eq!(count, 1, "bookings_current should still have 1 row; out={}", bc.raw());

    // cleanup extra payload rows (teardown_plan already covers plan_id tables, but ensure legacy bookings cleaned if needed)
    let _ = db_exec(&format!(
        "DELETE FROM bookings WHERE destination = '{dest}' AND hotel_name LIKE '%海論%' AND offer_id LIKE 'domestic:{plan}:%'"
    ));
}
