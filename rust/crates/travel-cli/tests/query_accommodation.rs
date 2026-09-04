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
fn query_accommodation_shows_three_jiufen_hotels() {
    let Some(_) = db_exec("SELECT 1") else {
        eprintln!("credless — skip");
        return;
    };
    let _ = run(&["db", "migrate"]);
    let n = common::nanos();
    let plan = format!("zz-accom-{n}");
    let dest = "jiufen";

    // Guard right after ids bound, before seed
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

    // Re-ensure seed after Guard (teardown may not delete domestic rows but destination-config is not plan-keyed; re-ensure)
    let fx = ensure_domestic_seed(n);

    let (ok, stdout, stderr) = run(&[
        "query-accommodation",
        "--dest",
        dest,
        "--date",
        "2026-09-03",
        "--plan-id",
        &plan,
    ]);
    if is_credless(&stderr) {
        eprintln!("credless on run — skip");
        return;
    }
    assert!(ok, "query-accommodation should succeed; stdout={stdout} stderr={stderr}");
    assert!(stdout.contains(&fx[0]), "should contain fixture 1: {stdout}");
    assert!(stdout.contains(&fx[1]), "should contain the test-only third row: {stdout}");
    assert!(stdout.contains(&fx[2]), "should contain fixture 3: {stdout}");
    // price/breakfast columns present
    assert!(stdout.contains("5200") && stdout.contains("7200") && stdout.contains("4200"), "prices: {stdout}");
    // header columns per spec
    assert!(stdout.contains("hotel_name") && stdout.contains("price_twd") && stdout.contains("breakfast"), "header: {stdout}");
}

#[test]
fn query_accommodation_sea_view_filter() {
    let Some(_) = db_exec("SELECT 1") else {
        eprintln!("credless — skip");
        return;
    };
    let _ = run(&["db", "migrate"]);
    let n = common::nanos();
    let plan = format!("zz-accom-sv-{n}");
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

    // All three seeded are sea_view=1; filtering should still return them.
    let (ok, stdout, stderr) = run(&[
        "query-accommodation",
        "--dest",
        dest,
        "--date",
        "2026-09-03",
        "--sea-view",
        "--plan-id",
        &plan,
    ]);
    if is_credless(&stderr) {
        eprintln!("credless on run — skip");
        return;
    }
    assert!(ok, "sea-view query should succeed; stdout={stdout} stderr={stderr}");
    assert!(stdout.contains(&fx[0]) && stdout.contains(&fx[1]) && stdout.contains(&fx[2]), "sea_view filter: {stdout}");

    // Hotel substring filter
    let (ok2, stdout2, stderr2) = run(&[
        "query-accommodation",
        "--dest",
        dest,
        "--date",
        "2026-09-03",
        "--hotel",
        fx[0].as_str(),
        "--plan-id",
        &plan,
    ]);
    if is_credless(&stderr2) {
        eprintln!("credless on run — skip");
        return;
    }
    assert!(ok2, "hotel filter should succeed; stdout={stdout2} stderr={stderr2}");
    assert!(stdout2.contains(&fx[0]), "hotel filter contains match: {stdout2}");
    assert!(!stdout2.contains(&fx[1]), "hotel filter excludes other: {stdout2}");
}

#[test]
fn query_accommodation_rejects_missing_dest_and_bad_date() {
    // Parse errors don't need Turso, but we still guard credless for env.
    let (ok, _out, err) = run(&["query-accommodation", "--date", "2026-09-03", "--plan-id", "any"]);
    assert!(!ok, "missing --dest should fail");
    assert!(err.contains("--dest") || err.contains("dest"), "err={err}");

    let (ok2, _out2, err2) = run(&[
        "query-accommodation",
        "--dest",
        "jiufen",
        "--date",
        "2026/09/03",
        "--plan-id",
        "any",
    ]);
    assert!(!ok2, "bad date should fail");
    assert!(err2.contains("Invalid --date"), "err2={err2}");
}
