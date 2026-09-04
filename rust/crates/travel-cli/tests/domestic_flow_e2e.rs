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

#[test]
fn domestic_jiufen_flow_query_then_book() {
    let Some(_) = db_exec("SELECT 1") else {
        eprintln!("credless — skip");
        return;
    };
    let _ = run(&["db", "migrate"]);
    let n = common::nanos();
    let plan = format!("test-domestic-e2e-jiufen-{n}");
    let dest = "jiufen";

    // Guard immediately after plan/dest bound.
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
            teardown_plan(&plan, &dest);
        }
    });

    // Re-ensure after pre-clean (destination_config is not plan-keyed; domestic rows survive teardown but keep idempotent).
    let fx = ensure_domestic_seed(n);

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
    assert!(stdout.contains(&fx[0]), "should contain fixture 1: {stdout}");
    assert!(stdout.contains(&fx[1]), "should contain the test-only third row: {stdout}");
    assert!(stdout.contains(&fx[2]), "should contain fixture 3: {stdout}");

    // 9. set-accommodation
    let (ok, stdout, stderr) = run(&[
        "set-accommodation",
        "--hotel",
        fx[0].as_str(),
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
        row.raw().contains(&fx[0]),
        "title should contain fixture 1: {}",
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
