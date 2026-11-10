//! Domestic flow-decision defer auto-skip integration tests (real Turso, Guard panic-safe).
//!
//! Verifies: `travel flow-decision shop mode --mode defer` for a domestic
//! destination auto-advances P3/P34 to skipped and leaves P4 accommodation
//! untouched (CLAUDE.md: "P4 accommodation is NOT skipped — a domestic trip
//! still books a stay"; realigned 2026-11-11 — it used to skip a pending P4):
//!   - when P4 is pending -> stays pending, no P4 status_changed
//!   - when P4 is booked -> preserved (not clobbered)

mod common;
use common::{bin, db_exec, is_credless, teardown_plan, Guard};
use std::process::Command;

// The commands under test write in ONE transaction; two concurrent transactions
// on the shared DB hit SQLITE_BUSY, so the DB tests in this file run one at a time.
static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// The booking-ledger rows set-accommodation writes are NOT plan-keyed
/// (bookings_current/payload keyed by `<plan>:` booking_key, legacy `bookings` by
/// `domestic:<plan>:` offer_id), so teardown_plan misses them — run before it.
fn teardown_bookings(plan: &str, dest: &str) {
    common::teardown_trip_bookings(plan);
    let _ = common::db_exec_teardown(&format!(
        "DELETE FROM bookings WHERE destination = '{dest}' AND offer_id LIKE 'domestic:{plan}:%'"
    ));
}

fn run(args: &[&str]) -> (bool, String, String) {
    let out = Command::new(bin()).args(args).output().expect("run travel");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// Seed three per-run fixture stays and return their hotel names.
///
/// Ids and names carry `n` because the tests in this file run in PARALLEL: they
/// used to share one fixture id, so the first test to finish ran its Guard and
/// deleted the rows the others were still reading. Names stay short — the CLI
/// table truncates hotel_name at 16 chars, and a truncated name fails `contains`.
fn ensure_domestic_seed(n: u128) -> [String; 3] {
    let sfx = n % 100_000_000;
    let names = [
        format!("ZZ海景一{sfx:08}"),
        format!("ZZ海景二{sfx:08}"),
        format!("ZZ海景三{sfx:08}"),
    ];
    let _ = db_exec(&format!(
        "INSERT OR IGNORE INTO domestic_accommodations \
         (id, destination, hotel_name, room_type, sea_view, price_twd, currency, breakfast_included, source, updated_at) \
         VALUES \
         ('zz_test_{n}_5200','jiufen','{}','海景雙人房',1,5200,'TWD',1,'manual',datetime('now')), \
         ('zz_test_{n}_7200','jiufen','{}','海景雙人房',1,7200,'TWD',1,'manual',datetime('now')), \
         ('zz_test_{n}_4200','jiufen','{}','海景雙人房',1,4200,'TWD',1,'manual',datetime('now'))",
        names[0], names[1], names[2]
    ));
    let _ = db_exec(
        "INSERT OR IGNORE INTO destination_config (slug, display_name, timezone, currency, language, origin) \
         VALUES ('jiufen','九份','Asia/Taipei','TWD','zh-TW','taiwan')",
    );
    names
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
fn domestic_defer_skips_p3_p34_and_leaves_p4_untouched() {
    let _lock = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let Some(_) = db_exec("SELECT 1") else {
        eprintln!("credless — skip");
        return;
    };
    let _ = run(&["db", "migrate"]);
    let n = common::nanos();
    let plan = format!("test-domestic-defer-{n}");
    let dest = "jiufen";

    teardown_plan(&plan, dest);
    let _g = Guard::new({
        let (plan, dest) = (plan.clone(), dest.to_string());
        move || {
            // The test-only third candidate is NOT plan-keyed, so teardown_plan
            // does not cover it — delete it explicitly or it leaks into shared Turso.
            // These fixture rows are NOT plan-keyed, so teardown_plan does not cover
            // them — delete them explicitly or they leak into shared Turso.
            let _ = common::db_exec_teardown(
                &format!("DELETE FROM domestic_accommodations WHERE id LIKE 'zz_test_{n}_%'"),
            );
            teardown_bookings(&plan, &dest);
            teardown_plan(&plan, &dest);
        }
    });
    let _fx = ensure_domestic_seed(n);

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
    assert!(stdout.contains("domestic shop deferred"), "hint present: {stdout}");
    assert!(stdout.contains("P4 accommodation untouched"), "hint names P4: {stdout}");

    // P3/P34 skipped; P4 untouched (still pending, no P4 status_changed).
    for pid in ["process_3_transportation", "process_3_4_packages"] {
        let st = process_status(&plan, dest, pid);
        assert_eq!(
            st.as_deref(),
            Some("skipped"),
            "{pid} should be skipped after defer; got {st:?}"
        );
    }
    let p4 = process_status(&plan, dest, "process_4_accommodation");
    assert_eq!(p4.as_deref(), Some("pending"), "P4 must stay pending after defer; got {p4:?}");
    let p4_events = db_exec(&format!(
        "SELECT COUNT(*) AS n FROM plan_events WHERE plan_id = '{plan}' \
           AND process_id = 'process_4_accommodation' AND event = 'status_changed'"
    ))
    .and_then(|r| r.scalar());
    assert_eq!(p4_events.as_deref(), Some("0"), "no P4 status_changed from defer");
    let hops = db_exec(&format!(
        "SELECT process_id || ':' || from_state || '->' || to_state AS v FROM plan_events \
         WHERE plan_id = '{plan}' AND scope = 'dest_process' AND event = 'status_changed' \
         ORDER BY process_id, sort_order"
    ))
    .expect("query hops");
    assert_eq!(
        hops.column(),
        vec![
            "process_3_4_packages:pending->skipped".to_string(),
            "process_3_transportation:pending->skipped".to_string(),
        ],
        "one logged hop per deferred process; out={hops}"
    );
}

#[test]
fn domestic_defer_preserves_booked_p4() {
    let _lock = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let Some(_) = db_exec("SELECT 1") else {
        eprintln!("credless — skip");
        return;
    };
    let _ = run(&["db", "migrate"]);
    let n = common::nanos();
    let plan = format!("test-domestic-defer-booked-{n}");
    let dest = "jiufen";

    teardown_plan(&plan, dest);
    let _g = Guard::new({
        let (plan, dest) = (plan.clone(), dest.to_string());
        move || {
            // The test-only third candidate is NOT plan-keyed, so teardown_plan
            // does not cover it — delete it explicitly or it leaks into shared Turso.
            // These fixture rows are NOT plan-keyed, so teardown_plan does not cover
            // them — delete them explicitly or they leak into shared Turso.
            let _ = common::db_exec_teardown(
                &format!("DELETE FROM domestic_accommodations WHERE id LIKE 'zz_test_{n}_%'"),
            );
            teardown_bookings(&plan, &dest);
            teardown_plan(&plan, &dest);
        }
    });
    let fx = ensure_domestic_seed(n);

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

    // Book accommodation first: set-accommodation on the fixture stay
    let (ok, stdout, stderr) = run(&[
        "set-accommodation",
        "--hotel",
        fx[0].as_str(),
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
        "DELETE FROM bookings WHERE destination = '{dest}' AND hotel_name LIKE '%{}%' AND offer_id LIKE 'domestic:{plan}:%'",
        fx[0]
    ));
}

#[test]
fn domestic_defer_leaves_booked_p3_alone() {
    let _lock = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let Some(_) = db_exec("SELECT 1") else {
        eprintln!("credless — skip");
        return;
    };
    let n = common::nanos();
    let plan = format!("zztest-defer-p3-{n}");
    let dest = "jiufen";
    let _g = Guard::new({
        let (plan, dest) = (plan.clone(), dest.to_string());
        move || teardown_plan(&plan, &dest)
    });

    let (ok, stdout, stderr) = run(&[
        "create-plan", &plan, "--dest", dest, "--start", "2026-11-01", "--end", "2026-11-02",
        "--airport", "TPE",
    ]);
    if is_credless(&stderr) {
        eprintln!("credless on create-plan — skip");
        return;
    }
    assert!(ok, "create-plan should succeed; stdout={stdout} stderr={stderr}");

    let (ok, stdout, stderr) = run(&[
        "set-process-status", "p3", "booked", "--dest", dest, "--plan-id", &plan,
    ]);
    assert!(ok, "p3 -> booked; stdout={stdout} stderr={stderr}");

    let (ok, stdout, stderr) = run(&[
        "flow-decision", "shop", "mode", "--mode", "defer", "--reason", "test-p3-booked",
        "--plan-id", &plan,
    ]);
    assert!(ok, "flow-decision defer should succeed; stdout={stdout} stderr={stderr}");
    assert!(
        stdout.contains("process_3_transportation stays booked"),
        "names the kept process: {stdout}"
    );

    // A booked P3 is never demoted to skipped; P3_4 still is.
    assert_eq!(process_status(&plan, dest, "process_3_transportation").as_deref(), Some("booked"));
    assert_eq!(process_status(&plan, dest, "process_3_4_packages").as_deref(), Some("skipped"));
    let p3_skips = db_exec(&format!(
        "SELECT COUNT(*) AS n FROM plan_events WHERE plan_id = '{plan}' \
           AND process_id = 'process_3_transportation' AND event = 'status_changed' \
           AND scope = 'dest_process' AND to_state = 'skipped'"
    ))
    .and_then(|r| r.scalar());
    assert_eq!(p3_skips.as_deref(), Some("0"), "no P3 -> skipped hop");
}
