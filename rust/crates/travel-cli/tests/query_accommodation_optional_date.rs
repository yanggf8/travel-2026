//! query-accommodation --date optional integration tests (real Turso, Guard panic-safe).
//!
//! Verifies: `query-accommodation --dest jiufen` works without --date (date shows "-"),
//! and with --date still lists correctly.

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
         ('zz_test_seaview_7200','jiufen','ZZ測試海景館','海景雙人房',1,7200,'TWD',1,'manual',datetime('now')), \
         ('jiufen_shancheng_seaview_4200','jiufen','山城逸境','海景雙人房',1,4200,'TWD',1,'manual',datetime('now'))",
    );
    let _ = db_exec(
        "INSERT OR IGNORE INTO destination_config (slug, display_name, timezone, currency, language, origin, lat, lon) \
         VALUES ('jiufen','九份','Asia/Taipei','TWD','zh-TW','taiwan',25.109,121.844)",
    );
}

#[test]
fn query_accommodation_without_date_shows_dash() {
    let Some(_) = db_exec("SELECT 1") else {
        eprintln!("credless — skip");
        return;
    };
    let _ = run(&["db", "migrate"]);
    ensure_domestic_seed();

    let n = common::nanos();
    let plan = format!("test-qacc-nodate-{n}");
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
    common::seed_plan(&plan, dest, 1);
    ensure_domestic_seed();

    // Without --date
    let (ok, stdout, stderr) = run(&[
        "query-accommodation",
        "--dest",
        dest,
        "--plan-id",
        &plan,
    ]);
    if is_credless(&stderr) {
        eprintln!("credless on query-accommodation — skip");
        return;
    }
    assert!(
        ok,
        "query-accommodation without --date should succeed; stdout={stdout} stderr={stderr}"
    );
    // Header should show date="-"
    assert!(
        stdout.contains("date=-") || stdout.contains("date= -") || stdout.contains("date=\"-\""),
        "header should show date=- when no --date given: {stdout}"
    );
    // Still lists hotels
    assert!(stdout.contains("海論"), "should contain 海論: {stdout}");
    assert!(stdout.contains("ZZ測試海景館"), "should contain the test-only third row: {stdout}");
    assert!(stdout.contains("山城逸境"), "should contain 山城逸境: {stdout}");
}

#[test]
fn query_accommodation_with_date_lists_normally() {
    let Some(_) = db_exec("SELECT 1") else {
        eprintln!("credless — skip");
        return;
    };
    let _ = run(&["db", "migrate"]);
    ensure_domestic_seed();

    let n = common::nanos();
    let plan = format!("test-qacc-withdate-{n}");
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
    common::seed_plan(&plan, dest, 1);
    ensure_domestic_seed();

    let (ok, stdout, stderr) = run(&[
        "query-accommodation",
        "--dest",
        dest,
        "--date",
        "2026-11-01",
        "--plan-id",
        &plan,
    ]);
    if is_credless(&stderr) {
        eprintln!("credless on query-accommodation — skip");
        return;
    }
    assert!(
        ok,
        "query-accommodation with --date should succeed; stdout={stdout} stderr={stderr}"
    );
    assert!(
        stdout.contains("2026-11-01"),
        "header should contain provided date: {stdout}"
    );
    assert!(stdout.contains("海論"), "should contain 海論: {stdout}");
    assert!(stdout.contains("5200"), "should contain price 5200: {stdout}");
}
