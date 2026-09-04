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
fn query_accommodation_without_date_shows_dash() {
    let Some(_) = db_exec("SELECT 1") else {
        eprintln!("credless — skip");
        return;
    };
    let _ = run(&["db", "migrate"]);
    let n = common::nanos();
    let plan = format!("test-qacc-nodate-{n}");
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
            teardown_plan(&plan, &dest);
        }
    });
    common::seed_plan(&plan, dest, 1);
    let fx = ensure_domestic_seed(n);

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
    assert!(stdout.contains(&fx[0]), "should contain fixture 1: {stdout}");
    assert!(stdout.contains(&fx[1]), "should contain the test-only third row: {stdout}");
    assert!(stdout.contains(&fx[2]), "should contain fixture 3: {stdout}");
}

#[test]
fn query_accommodation_with_date_lists_normally() {
    let Some(_) = db_exec("SELECT 1") else {
        eprintln!("credless — skip");
        return;
    };
    let _ = run(&["db", "migrate"]);
    let n = common::nanos();
    let plan = format!("test-qacc-withdate-{n}");
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
            teardown_plan(&plan, &dest);
        }
    });
    common::seed_plan(&plan, dest, 1);
    let fx = ensure_domestic_seed(n);

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
    assert!(stdout.contains(&fx[0]), "should contain fixture 1: {stdout}");
    assert!(stdout.contains("5200"), "should contain price 5200: {stdout}");
}
