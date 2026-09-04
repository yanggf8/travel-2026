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
        "--hotel",
        "任一飯店",
        "--room-type",
        "海景雙人房",
        "--plan-id",
        "any",
    ]);
    assert!(!ok, "missing --price should fail");
    assert!(err.contains("--price"), "err={err}");

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
