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

#[test]
fn query_accommodation_shows_three_jiufen_hotels() {
    let Some(_) = db_exec("SELECT 1") else {
        eprintln!("credless — skip");
        return;
    };
    let _ = run(&["db", "migrate"]);
    ensure_domestic_seed();

    let n = common::nanos();
    let plan = format!("zz-accom-{n}");
    let dest = "jiufen";

    // Guard right after ids bound, before seed
    let _g = Guard::new({
        let (plan, dest) = (plan.clone(), dest.to_string());
        move || teardown_plan(&plan, &dest)
    });
    teardown_plan(&plan, dest);
    common::seed_plan(&plan, dest, 1);

    // Re-ensure seed after Guard (teardown may not delete domestic rows but destination-config is not plan-keyed; re-ensure)
    ensure_domestic_seed();

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
    assert!(stdout.contains("海論"), "should contain 海論: {stdout}");
    assert!(stdout.contains("CHLIV"), "should contain CHLIV: {stdout}");
    assert!(stdout.contains("山城逸境"), "should contain 山城逸境: {stdout}");
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
    ensure_domestic_seed();

    let n = common::nanos();
    let plan = format!("zz-accom-sv-{n}");
    let dest = "jiufen";
    let _g = Guard::new({
        let (plan, dest) = (plan.clone(), dest.to_string());
        move || teardown_plan(&plan, &dest)
    });
    teardown_plan(&plan, dest);
    common::seed_plan(&plan, dest, 1);
    ensure_domestic_seed();

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
    assert!(stdout.contains("海論") && stdout.contains("CHLIV") && stdout.contains("山城逸境"), "sea_view filter: {stdout}");

    // Hotel substring filter
    let (ok2, stdout2, stderr2) = run(&[
        "query-accommodation",
        "--dest",
        dest,
        "--date",
        "2026-09-03",
        "--hotel",
        "海論",
        "--plan-id",
        &plan,
    ]);
    if is_credless(&stderr2) {
        eprintln!("credless on run — skip");
        return;
    }
    assert!(ok2, "hotel filter should succeed; stdout={stdout2} stderr={stderr2}");
    assert!(stdout2.contains("海論"), "hotel filter contains match: {stdout2}");
    assert!(!stdout2.contains("CHLIV"), "hotel filter excludes other: {stdout2}");
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
