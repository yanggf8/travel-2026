mod common;
use common::{bin, db_exec, is_credless, teardown_plan, Guard};
use std::process::Command;

fn run(args: &[&str]) -> (bool, String, String) {
    let out = Command::new(bin())
        .args(args)
        .output()
        .expect("run travel");
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
    // If row already existed without coords, fill them.
    let _ = db_exec(
        "UPDATE destination_config SET lat = 25.109, lon = 121.844 \
         WHERE slug = 'jiufen' AND (lat IS NULL OR lon IS NULL)",
    );
    let _ = db_exec(
        "INSERT OR IGNORE INTO domestic_accommodations \
         (id, destination, hotel_name, room_type, sea_view, price_twd, currency, breakfast_included, source, updated_at) \
         VALUES \
         ('jiufen_hailun_seaview_5200','jiufen','海論','海景雙人房',1,5200,'TWD',1,'manual',datetime('now')), \
         ('jiufen_chliv_seaview_7200','jiufen','CHLIV','海景雙人房',1,7200,'TWD',1,'manual',datetime('now')), \
         ('jiufen_shancheng_seaview_4200','jiufen','山城逸境','海景雙人房',1,4200,'TWD',1,'manual',datetime('now'))",
    );
}

#[test]
fn domestic_jiufen_flow_query_then_book() {
    let Some(_) = db_exec("SELECT 1") else {
        eprintln!("credless — skip");
        return;
    };
    let _ = run(&["db", "migrate"]);
    ensure_domestic_seed();

    let n = common::nanos();
    let plan = format!("test-domestic-e2e-jiufen-{n}");
    let dest = "jiufen";

    // Guard immediately after plan/dest bound.
    teardown_plan(&plan, dest);
    let _g = Guard::new({
        let (plan, dest) = (plan.clone(), dest.to_string());
        move || teardown_plan(&plan, &dest)
    });

    // Re-ensure after pre-clean (destination_config is not plan-keyed; domestic rows survive teardown but keep idempotent).
    ensure_domestic_seed();

    // 4. create-plan
    let (ok, stdout, stderr) = run(&[
        "create-plan",
        &plan,
        "--dest",
        dest,
        "--start",
        "2026-10-12",
        "--end",
        "2026-10-13",
        "--airport",
        "TPE",
    ]);
    if is_credless(&stderr) {
        eprintln!("credless on create-plan — skip");
        return;
    }
    assert!(ok, "create-plan should succeed; stdout={stdout} stderr={stderr}");

    // 5. flow-decision shop mode --mode defer
    let (ok, stdout, stderr) = run(&[
        "flow-decision",
        "shop",
        "mode",
        "--mode",
        "defer",
        "--reason",
        "domestic e2e",
        "--plan-id",
        &plan,
    ]);
    if is_credless(&stderr) {
        eprintln!("credless on flow-decision — skip");
        return;
    }
    assert!(ok, "flow-decision should succeed; stdout={stdout} stderr={stderr}");

    // 6. set-process-status p3 skipped and p34 skipped
    let (ok, stdout, stderr) = run(&["set-process-status", "p3", "skipped", "--plan-id", &plan]);
    if is_credless(&stderr) {
        eprintln!("credless on set-process-status p3 — skip");
        return;
    }
    assert!(ok, "set-process-status p3 skipped should succeed; stdout={stdout} stderr={stderr}");

    let (ok, stdout, stderr) = run(&["set-process-status", "p34", "skipped", "--plan-id", &plan]);
    if is_credless(&stderr) {
        eprintln!("credless on set-process-status p34 — skip");
        return;
    }
    assert!(ok, "set-process-status p34 skipped should succeed; stdout={stdout} stderr={stderr}");

    // 7. scaffold-itinerary
    let (ok, stdout, stderr) = run(&["scaffold-itinerary", "--plan-id", &plan]);
    if is_credless(&stderr) {
        eprintln!("credless on scaffold-itinerary — skip");
        return;
    }
    assert!(
        ok,
        "scaffold-itinerary should succeed; stdout={stdout} stderr={stderr}"
    );

    // 8. query-accommodation --dest jiufen --date 2026-10-12 --plan-id <plan>
    let (ok, stdout, stderr) = run(&[
        "query-accommodation",
        "--dest",
        dest,
        "--date",
        "2026-10-12",
        "--plan-id",
        &plan,
    ]);
    if is_credless(&stderr) {
        eprintln!("credless on query-accommodation — skip");
        return;
    }
    assert!(
        ok,
        "query-accommodation should succeed; stdout={stdout} stderr={stderr}"
    );
    assert!(stdout.contains("海論"), "should contain 海論: {stdout}");
    assert!(stdout.contains("CHLIV"), "should contain CHLIV: {stdout}");
    assert!(stdout.contains("山城逸境"), "should contain 山城逸境: {stdout}");

    // 9. set-accommodation
    let (ok, stdout, stderr) = run(&[
        "set-accommodation",
        "--hotel",
        "海論",
        "--room-type",
        "海景雙人房",
        "--price",
        "5200",
        "--date",
        "2026-10-12",
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

    // 10. Verify bookings_current has 1 row and process statuses.
    let bc = db_exec(&format!(
        "SELECT COUNT(*) AS n FROM bookings_current WHERE trip_id = '{plan}'"
    ))
    .expect("query bookings_current count");
    let count: i64 = bc
        .raw()
        .lines()
        .find_map(|l| l.split(':').nth(1))
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(-1);
    assert_eq!(count, 1, "bookings_current should have 1 row; out={}", bc.raw());

    let row = db_exec(&format!(
        "SELECT title AS t FROM bookings_current WHERE trip_id = '{plan}'"
    ))
    .unwrap();
    assert!(
        row.raw().contains("海論"),
        "title should contain 海論: {}",
        row.raw()
    );

    let ps3 = db_exec(&format!(
        "SELECT status AS v FROM process_statuses WHERE plan_id = '{plan}' AND destination = '{dest}' AND process_id = 'process_3_transportation'"
    ))
    .expect("query p3 status");
    assert!(
        ps3.raw().contains("skipped"),
        "process_3_transportation should be skipped; out={}",
        ps3.raw()
    );

    let ps4 = db_exec(&format!(
        "SELECT status AS v FROM process_statuses WHERE plan_id = '{plan}' AND destination = '{dest}' AND process_id = 'process_4_accommodation'"
    ))
    .expect("query p4 status");
    assert!(
        ps4.raw().contains("booked"),
        "process_4_accommodation should be booked; out={}",
        ps4.raw()
    );
}
