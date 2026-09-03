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
         ('jiufen_chliv_seaview_7200','jiufen','CHLIV','海景雙人房',1,7200,'TWD',1,'manual',datetime('now')), \
         ('jiufen_shancheng_seaview_4200','jiufen','山城逸境','海景雙人房',1,4200,'TWD',1,'manual',datetime('now'))",
    );
    let _ = db_exec(
        "INSERT OR IGNORE INTO destination_config (slug, display_name, timezone, currency, language, origin) \
         VALUES ('jiufen','九份','Asia/Taipei','TWD','zh-TW','taiwan')",
    );
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
    ensure_domestic_seed();

    let n = common::nanos();
    let plan = format!("zz-setacc-{n}");
    let dest = "jiufen";

    let _g = Guard::new({
        let (plan, dest) = (plan.clone(), dest.to_string());
        move || teardown_plan(&plan, &dest)
    });
    teardown_plan(&plan, dest);
    common::seed_plan(&plan, dest, 1);
    ensure_domestic_seed();
    seed_p4_pending(&plan, dest);

    // Clean any prior bookings for this plan
    let _ = db_exec(&format!("DELETE FROM bookings_current WHERE trip_id = '{plan}'"));
    let _ = db_exec(&format!("DELETE FROM bookings_current_payload WHERE booking_key LIKE '{plan}:%'"));
    let _ = db_exec(&format!("DELETE FROM bookings WHERE destination = '{dest}' AND offer_id LIKE 'domestic:{plan}:%'"));

    let (ok, stdout, stderr) = run(&[
        "set-accommodation",
        "--hotel",
        "海論",
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
    assert!(stdout.contains("海論") && stdout.contains("海景雙人房"), "hotel/room in output: {stdout}");
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
    assert!(row.raw().contains("海論 海景雙人房"), "title format '海論 海景雙人房': {}", row.raw());

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
    ensure_domestic_seed();
    let n = common::nanos();
    let plan = format!("zz-setacc-unk-{n}");
    let dest = "jiufen";
    let _g = Guard::new({
        let (plan, dest) = (plan.clone(), dest.to_string());
        move || teardown_plan(&plan, &dest)
    });
    teardown_plan(&plan, dest);
    common::seed_plan(&plan, dest, 1);
    ensure_domestic_seed();
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
        "海論",
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
        "海論",
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
