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

fn seed_p4_pending(plan: &str, dest: &str) {
    let _ = db_exec(&format!(
        "INSERT OR REPLACE INTO process_statuses (plan_id, destination, process_id, status, updated_at) \
         VALUES ('{plan}','{dest}','process_3_transportation','skipped',datetime('now')); \
         INSERT OR REPLACE INTO process_statuses (plan_id, destination, process_id, status, updated_at) \
         VALUES ('{plan}','{dest}','process_4_accommodation','pending',datetime('now'))"
    ));
}

#[test]
fn set_accommodation_books_and_advances_p4() {
    let _lock = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let Some(_) = db_exec("SELECT 1") else {
        eprintln!("credless — skip");
        return;
    };
    let _ = run(&["db", "migrate"]);
    let n = common::nanos();
    let plan = format!("zz-setacc-{n}");
    let dest = "jiufen";

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
    teardown_plan(&plan, dest);
    common::seed_plan(&plan, dest, 1);
    let fx = ensure_domestic_seed(n);
    seed_p4_pending(&plan, dest);

    // Clean any prior bookings for this plan
    let _ = db_exec(&format!("DELETE FROM bookings_current WHERE trip_id = '{plan}'"));
    let _ = db_exec(&format!("DELETE FROM bookings_current_payload WHERE booking_key LIKE '{plan}:%'"));
    let _ = db_exec(&format!("DELETE FROM bookings WHERE destination = '{dest}' AND offer_id LIKE 'domestic:{plan}:%'"));

    let (ok, stdout, stderr) = run(&[
        "set-accommodation",
        "--hotel",
        fx[0].as_str(),
        "--room-type",
        "海景雙人房",
        "--price",
        "5200",
        "--date",
        "2026-09-03",
        "--dest",
        dest,
        "--plan-id",
        &plan,
    ]);
    if is_credless(&stderr) {
        eprintln!("credless on run — skip");
        return;
    }
    assert!(ok, "set-accommodation should succeed; stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("✅ Booked accommodation:"), "output: {stdout}");
    assert!(stdout.contains(&fx[0]) && stdout.contains("海景雙人房"), "hotel/room in output: {stdout}");
    assert!(stdout.contains("5200"), "price in output: {stdout}");
    assert!(stdout.contains(dest), "dest in output: {stdout}");
    assert!(stdout.contains(&plan), "plan in output: {stdout}");

    // Assert bookings_current has 1 row
    let bc = db_exec(&format!("SELECT COUNT(*) AS n FROM bookings_current WHERE trip_id = '{plan}'")).expect("query bookings_current");
    let count_str = bc.raw();
    // Parse count value after ':'
    let count: i64 = count_str.lines().find_map(|l| l.split(':').nth(1)).and_then(|v| v.trim().parse().ok()).unwrap_or(-1);
    assert_eq!(count, 1, "bookings_current should have 1 row; out={count_str}");

    // Assert title + price
    let row = db_exec(&format!("SELECT title AS t FROM bookings_current WHERE trip_id = '{plan}'")).unwrap();
    assert!(row.raw().contains(&format!("{} 海景雙人房", fx[0])), "title format '<hotel> <room>': {}", row.raw());

    // Assert P4 is booked
    let ps = db_exec(&format!("SELECT status AS v FROM process_statuses WHERE plan_id = '{plan}' AND destination = '{dest}' AND process_id = 'process_4_accommodation'")).unwrap();
    let status = ps.raw();
    assert!(status.contains("booked"), "P4 should be booked; out={status}");

    // Cleanup handled by Guard
}

#[test]
fn set_accommodation_rejects_unknown_hotel() {
    let _lock = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let Some(_) = db_exec("SELECT 1") else {
        eprintln!("credless — skip");
        return;
    };
    let _ = run(&["db", "migrate"]);
    let n = common::nanos();
    let plan = format!("zz-setacc-unk-{n}");
    let dest = "jiufen";
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
    teardown_plan(&plan, dest);
    common::seed_plan(&plan, dest, 1);
    let _fx = ensure_domestic_seed(n);
    seed_p4_pending(&plan, dest);

    let (ok, _out, err) = run(&[
        "set-accommodation",
        "--hotel",
        "不存在飯店",
        "--room-type",
        "海景雙人房",
        "--price",
        "5200",
        "--dest",
        dest,
        "--plan-id",
        &plan,
    ]);
    if is_credless(&err) {
        eprintln!("credless — skip");
        return;
    }
    assert!(!ok, "unknown hotel should fail");
    assert!(err.contains("No accommodation found") || err.contains("query-accommodation"), "hint: {err}");
}

#[test]
fn set_accommodation_validates_required_flags() {
    // No DB needed — parse errors before connect
    let (ok, _out, err) = run(&[
        "set-accommodation",
        "--room-type",
        "海景雙人房",
        "--plan-id",
        "any",
    ]);
    assert!(!ok, "missing --hotel should fail");
    assert!(err.contains("--hotel"), "err={err}");

    let (ok2, _out2, err2) = run(&[
        "set-accommodation",
        "--hotel",
        "任一飯店",
        "--room-type",
        "海景雙人房",
        "--price",
        "notanint",
        "--plan-id",
        "any",
    ]);
    assert!(!ok2, "non-int price should fail");
    assert!(err2.contains("--price"), "err2={err2}");
}

fn scalar(sql: &str) -> Option<String> {
    db_exec(sql).and_then(|r| r.scalar())
}

/// Re-booking a stay whose P4 is already booked (e.g. a corrected price) has no
/// status hop, but still writes the `accommodation_booking_updated` event — the
/// audit gap this closes. A P4 already confirmed is past booked and is refused.
#[test]
fn set_accommodation_on_booked_p4_writes_the_booking_change_event() {
    let _lock = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    if db_exec("SELECT 1").is_none() {
        return;
    }
    let n = common::nanos();
    let plan = format!("zz_test_{n}_setacc_upd");
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
    common::seed_plan(&plan, dest, 1);
    let fx = ensure_domestic_seed(n);
    seed_p4_pending(&plan, dest);
    let book = |price: &str| {
        run(&[
            "set-accommodation", "--hotel", fx[0].as_str(), "--room-type", "海景雙人房",
            "--price", price, "--date", "2026-09-03", "--dest", dest, "--plan-id", &plan,
        ])
    };

    let (ok, stdout, stderr) = book("5200");
    if is_credless(&stderr) {
        return;
    }
    assert!(ok, "first booking; stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("pending → populated → booking → booked"), "path printed: {stdout}");

    let (ok, stdout, stderr) = book("5300");
    assert!(ok, "price correction on a booked P4; stdout={stdout} stderr={stderr}");

    let status_changes = scalar(&format!(
        "SELECT COUNT(*) AS n FROM plan_events WHERE plan_id = '{plan}' AND scope = 'dest_process' \
           AND process_id = 'process_4_accommodation' AND event = 'status_changed'"
    ));
    assert_eq!(status_changes.as_deref(), Some("3"), "only the first booking moved P4");
    let prices = db_exec(&format!(
        "SELECT d.value AS v FROM plan_event_data d JOIN plan_events e \
           ON e.plan_id = d.plan_id AND e.scope = d.scope AND e.destination = d.destination \
          AND e.process_id = d.process_id AND e.sort_order = d.sort_order \
         WHERE d.plan_id = '{plan}' AND d.scope = 'dest_process' \
           AND e.event = 'accommodation_booking_updated' AND d.key = 'price' ORDER BY d.sort_order"
    ))
    .expect("event kv");
    assert_eq!(prices.column(), vec!["5200", "5300"], "one booking-change event per run; out={prices}");
    let kv_keys = db_exec(&format!(
        "SELECT DISTINCT d.key AS v FROM plan_event_data d JOIN plan_events e \
           ON e.plan_id = d.plan_id AND e.scope = d.scope AND e.destination = d.destination \
          AND e.process_id = d.process_id AND e.sort_order = d.sort_order \
         WHERE d.plan_id = '{plan}' AND e.event = 'accommodation_booking_updated' ORDER BY d.key"
    ))
    .expect("event kv keys");
    assert_eq!(kv_keys.column(), vec!["date", "hotel", "price"], "out={kv_keys}");
    assert_eq!(
        scalar(&format!("SELECT price_amount AS v FROM bookings_current WHERE trip_id = '{plan}'")).as_deref(),
        Some("5300"),
        "booking row carries the corrected price"
    );

    // Confirmed is past booked: refused, nothing written.
    let (ok, stdout, stderr) =
        run(&["set-process-status", "p4", "confirmed", "--dest", dest, "--plan-id", &plan]);
    assert!(ok, "booked -> confirmed; stdout={stdout} stderr={stderr}");
    let events_before = scalar(&format!("SELECT COUNT(*) AS n FROM plan_events WHERE plan_id = '{plan}'"));
    let (ok, stdout, stderr) = book("5400");
    assert!(!ok, "a confirmed P4 must refuse set-accommodation; stdout={stdout}");
    assert!(stderr.contains("already confirmed"), "stderr={stderr}");
    assert_eq!(
        scalar(&format!("SELECT COUNT(*) AS n FROM plan_events WHERE plan_id = '{plan}'")),
        events_before,
        "nothing written"
    );
    assert_eq!(
        scalar(&format!("SELECT price_amount AS v FROM bookings_current WHERE trip_id = '{plan}'")).as_deref(),
        Some("5300"),
        "booking row untouched"
    );
}
