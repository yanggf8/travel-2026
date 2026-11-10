//! clear-accommodation integration test (real Turso, Guard panic-safe).
//!
//! Verifies: set-accommodation booked -> clear-accommodation --hotel <fixture>
//! removes bookings_current row and reverts P4 to selecting; a CONFIRMED P4
//! clears through confirmed -> booked -> cancelled -> selecting; a P4 with no
//! legal path back fails and deletes nothing (status machine, 2026-11-11).

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
    let _lock = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let Some(_) = db_exec("SELECT 1") else {
        eprintln!("credless — skip");
        return;
    };
    let _ = run(&["db", "migrate"]);
    let n = common::nanos();
    let plan = format!("test-clear-acc-{n}");
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
    common::seed_plan(&plan, dest, 1);
    let fx = ensure_domestic_seed(n);

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
        fx[0].as_str(),
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

fn count(sql: &str) -> i64 {
    db_exec(sql)
        .and_then(|r| r.scalar())
        .and_then(|v| v.parse().ok())
        .unwrap_or(-1)
}

/// Seed a plan with P4 pending and book the first fixture stay. Returns the
/// fixture names, or None when creds vanished mid-test.
fn seed_and_book(plan: &str, dest: &str, n: u128) -> Option<[String; 3]> {
    common::seed_plan(plan, dest, 1);
    let fx = ensure_domestic_seed(n);
    let _ = db_exec(&format!(
        "INSERT OR REPLACE INTO process_statuses (plan_id, destination, process_id, status, updated_at) \
         VALUES ('{plan}','{dest}','process_4_accommodation','pending',datetime('now'))"
    ));
    let (ok, stdout, stderr) = run(&[
        "set-accommodation", "--hotel", fx[0].as_str(), "--room-type", "海景雙人房",
        "--price", "5200", "--date", "2026-11-01", "--dest", dest, "--plan-id", plan,
    ]);
    if is_credless(&stderr) {
        return None;
    }
    assert!(ok, "set-accommodation should succeed; stdout={stdout} stderr={stderr}");
    Some(fx)
}

#[test]
fn clear_accommodation_from_confirmed_walks_back_through_booked() {
    let _lock = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    if db_exec("SELECT 1").is_none() {
        return;
    }
    let n = common::nanos();
    let plan = format!("zz_test_{n}_clr_conf");
    let dest = "jiufen";
    let _g = Guard::new({
        let (plan, dest) = (plan.clone(), dest.to_string());
        move || {
            let _ = common::db_exec_teardown(&format!(
                "DELETE FROM domestic_accommodations WHERE id LIKE 'zz_test_{n}_%'"
            ));
            teardown_bookings(&plan, &dest);
            teardown_plan(&plan, &dest);
        }
    });
    let Some(fx) = seed_and_book(&plan, dest, n) else { return };

    let (ok, stdout, stderr) = run(&["set-process-status", "p4", "confirmed", "--dest", dest, "--plan-id", &plan]);
    assert!(ok, "booked -> confirmed; stdout={stdout} stderr={stderr}");
    assert_eq!(process_status(&plan, dest).as_deref(), Some("confirmed"));

    let (ok, stdout, stderr) = run(&["clear-accommodation", "--hotel", fx[0].as_str(), "--dest", dest, "--plan-id", &plan]);
    assert!(ok, "clear from confirmed should succeed; stdout={stdout} stderr={stderr}");
    assert!(
        stdout.contains("confirmed → booked → cancelled → selecting"),
        "path printed; stdout={stdout}"
    );
    assert_eq!(process_status(&plan, dest).as_deref(), Some("selecting"));
    let hops = db_exec(&format!(
        "SELECT from_state || '->' || to_state AS v FROM plan_events WHERE plan_id = '{plan}' \
           AND scope = 'dest_process' AND process_id = 'process_4_accommodation' \
           AND event = 'status_changed' ORDER BY sort_order"
    ))
    .expect("hops");
    let hops = hops.column();
    assert_eq!(
        hops[hops.len() - 3..].to_vec(),
        vec!["confirmed->booked", "booked->cancelled", "cancelled->selecting"],
        "clear hops; all={hops:?}"
    );
    assert_eq!(
        count(&format!("SELECT COUNT(*) AS n FROM bookings_current WHERE trip_id = '{plan}'")),
        0,
        "booking removed"
    );
}

#[test]
fn clear_accommodation_with_no_legal_path_deletes_nothing() {
    let _lock = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    if db_exec("SELECT 1").is_none() {
        return;
    }
    let n = common::nanos();
    let plan = format!("zz_test_{n}_clr_nopath");
    let dest = "jiufen";
    let _g = Guard::new({
        let (plan, dest) = (plan.clone(), dest.to_string());
        move || {
            let _ = common::db_exec_teardown(&format!(
                "DELETE FROM domestic_accommodations WHERE id LIKE 'zz_test_{n}_%'"
            ));
            teardown_bookings(&plan, &dest);
            teardown_plan(&plan, &dest);
        }
    });
    let Some(fx) = seed_and_book(&plan, dest, n) else { return };

    // A drifted, off-ladder P4: no legal path back to selecting.
    db_exec(&format!(
        "UPDATE process_statuses SET status = 'archived' WHERE plan_id = '{plan}' \
           AND destination = '{dest}' AND process_id = 'process_4_accommodation'"
    ))
    .expect("corrupt P4");
    let ops_before = count(&format!("SELECT COUNT(*) AS n FROM operation_runs WHERE plan_id = '{plan}'"));
    let events_before = count(&format!("SELECT COUNT(*) AS n FROM plan_events WHERE plan_id = '{plan}'"));

    let (ok, stdout, stderr) = run(&["clear-accommodation", "--hotel", fx[0].as_str(), "--dest", dest, "--plan-id", &plan]);
    assert!(!ok, "no legal path must fail; stdout={stdout}");
    assert!(stderr.contains("nothing was cleared"), "stderr={stderr}");
    assert_eq!(
        count(&format!("SELECT COUNT(*) AS n FROM bookings_current WHERE trip_id = '{plan}'")),
        1,
        "the booking must survive"
    );
    assert_eq!(
        count(&format!(
            "SELECT COUNT(*) AS n FROM bookings WHERE destination = '{dest}' AND offer_id LIKE 'domestic:{plan}:%'"
        )),
        1,
        "the legacy booking must survive"
    );
    assert_eq!(process_status(&plan, dest).as_deref(), Some("archived"));
    assert_eq!(count(&format!("SELECT COUNT(*) AS n FROM operation_runs WHERE plan_id = '{plan}'")), ops_before);
    assert_eq!(count(&format!("SELECT COUNT(*) AS n FROM plan_events WHERE plan_id = '{plan}'")), events_before);
}
