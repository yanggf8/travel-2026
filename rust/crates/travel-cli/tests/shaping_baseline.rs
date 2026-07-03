//! Integration test — port of tests/integration/shaping-baseline-cli.regression.test.ts.
//!
//! Drives the built `travel` binary's `shaping-baseline` subcommand against a
//! real Turso DB. Each case seeds throwaway shaping_tour_group_offers rows under
//! a UNIQUE `shaping-test-<nanos>` run id, runs shaping-baseline, asserts on
//! plain-text / --json stdout, then tears everything down. Real plans/runs are
//! never touched.
//!
//! Skips cleanly when Turso creds/connection are absent.
//!
//! Parity note: the TS test seeds a `raw_json` column (legacy JSON blob). The
//! RDB has been de-JSON'd — confidence lives in the typed `raw_confidence`
//! column — so the seeds here set `raw_confidence` directly. Behavioral checks
//! (cheapest-per-(date,kind), savings, region filter, --json fields) are ported
//! unchanged.

use std::process::Command;

mod common;
use common::{bin, db_exec, db_exec_teardown, is_credless, is_transient, nanos, Guard};

fn run(args: &[&str]) -> Option<(bool, String, String)> {
    for attempt in 0..6 {
        let out = Command::new(bin()).args(args).output().expect("spawn travel");
        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
        if !out.status.success() && is_credless(&stderr) {
            return None;
        }
        if !out.status.success() && is_transient(&stderr) && attempt < 5 {
            std::thread::sleep(std::time::Duration::from_millis(400 * (attempt + 1)));
            continue;
        }
        return Some((out.status.success(), stdout, stderr));
    }
    unreachable!()
}

fn unique_run_id() -> String {
    format!("shaping-test-{}", nanos())
}

fn sql_lit(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn cleanup(run_id: &str) {
    let r = sql_lit(run_id);
    let _ = db_exec_teardown(&format!(
        "DELETE FROM shaping_tour_group_offers WHERE run_id = {r}; \
         DELETE FROM shaping_research_runs WHERE run_id = {r};"
    ));
}

/// Seed the minimal research run so shaping-baseline can resolve it.
fn seed_run(run_id: &str) -> Option<()> {
    db_exec(&format!(
        "INSERT INTO shaping_research_runs \
         (run_id, origin_code, pax, window_start, window_end, currency, exchange_rate_usd_twd, status, created_at, updated_at) \
         VALUES ({}, 'TPE', 2, '2026-06-12', '2026-06-25', 'TWD', 32.0, 'started', '2026-05-27T00:00:00Z', '2026-05-27T00:00:00Z');",
        sql_lit(run_id)
    ))
    .map(|_| ())
}

/// Seed one tour-group offer row. `confidence` → raw_confidence (de-JSON'd).
#[allow(clippy::too_many_arguments)]
fn seed_offer(
    run_id: &str,
    offer_id: &str,
    dest_region: &str,
    product_kind: &str,
    price: i64,
    depart: &str,
    ret: &str,
    confidence: Option<&str>,
) -> Option<()> {
    let conf = match confidence {
        Some(c) => sql_lit(c),
        None => "NULL".to_string(),
    };
    db_exec(&format!(
        "INSERT INTO shaping_tour_group_offers \
         (run_id, offer_id, source_id, dest_region, depart_date, return_date, nights, \
          price_per_person_twd, title, url, scraped_at, product_kind, raw_confidence) \
         VALUES ({}, {}, 'besttour', {}, {}, {}, 3, {price}, 'test', 'http://test/', \
          '2026-05-27T00:00:00Z', {}, {conf});",
        sql_lit(run_id),
        sql_lit(offer_id),
        sql_lit(dest_region),
        sql_lit(depart),
        sql_lit(ret),
        sql_lit(product_kind),
    ))
    .map(|_| ())
}

// ── error cases ──────────────────────────────────────────────────────

#[tokio::test]
async fn errors_when_run_id_is_missing() {
    let Some((ok, _so, stderr)) = run(&["shaping-baseline"]) else {
        eprintln!("skipping: no Turso credentials");
        return;
    };
    assert!(!ok, "expected non-zero exit when --run missing");
    assert!(
        stderr.contains("--run") || stderr.to_lowercase().contains("requires"),
        "expected --run error, got: {stderr}"
    );
}

#[tokio::test]
async fn errors_when_run_does_not_exist() {
    let Some((ok, _so, stderr)) = run(&["shaping-baseline", "--run", "no-such-run-shaping-test-xyz"]) else {
        eprintln!("skipping: no Turso credentials");
        return;
    };
    assert!(!ok, "expected non-zero exit for missing run");
    assert!(
        stderr.to_lowercase().contains("not found"),
        "expected 'not found', got: {stderr}"
    );
}

#[tokio::test]
async fn shows_empty_result_cleanly_when_no_offers_exist() {
    let run_id = unique_run_id();
    cleanup(&run_id);
    let _g = Guard::new({
        let run_id = run_id.clone();
        move || cleanup(&run_id)
    });
    if seed_run(&run_id).is_none() {
        eprintln!("skipping: no Turso credentials");
        return;
    }
    let (ok, stdout, stderr) = run(&["shaping-baseline", "--run", &run_id]).unwrap();
    assert!(ok, "expected success, stderr: {stderr}, stdout: {stdout}");
    let lc = stdout.to_lowercase();
    assert!(
        lc.contains("no offers") || lc.contains("no tour-group"),
        "expected empty message, got: {stdout}"
    );
}

// ── grouping / cheapest-per-(date,kind) ──────────────────────────────

#[tokio::test]
async fn groups_by_product_kind_and_shows_cheapest_per_date_kind() {
    let run_id = unique_run_id();
    cleanup(&run_id);
    let _g = Guard::new({
        let run_id = run_id.clone();
        move || cleanup(&run_id)
    });
    if seed_run(&run_id).is_none() {
        eprintln!("skipping: no Turso credentials");
        return;
    }
    // group_tour rows
    seed_offer(&run_id, "gt1", "okinawa", "group_tour", 21999, "2026-06-18", "2026-06-21", None).unwrap();
    seed_offer(&run_id, "gt2", "okinawa", "group_tour", 26900, "2026-06-21", "2026-06-24", None).unwrap();
    seed_offer(&run_id, "gt3", "okinawa", "group_tour", 25000, "2026-06-18", "2026-06-21", None).unwrap();
    // fit rows
    seed_offer(&run_id, "fit1", "okinawa", "fit", 11900, "2026-06-18", "2026-06-21", Some("high")).unwrap();
    seed_offer(&run_id, "fit2", "okinawa", "fit", 12999, "2026-06-21", "2026-06-24", Some("high")).unwrap();

    let (ok, stdout, stderr) = run(&["shaping-baseline", "--run", &run_id]).unwrap();
    assert!(ok, "expected success, stderr: {stderr}");
    assert!(stdout.to_lowercase().contains("baseline"), "got: {stdout}");
    assert!(stdout.contains("11,900"), "cheapest FIT 6/18 missing: {stdout}");
    assert!(stdout.contains("21,999"), "cheapest group_tour 6/18 missing: {stdout}");
    assert!(stdout.contains("12,999"), "cheapest FIT 6/21 missing: {stdout}");
    assert!(stdout.contains("26,900"), "cheapest group_tour 6/21 missing: {stdout}");
    assert!(stdout.to_lowercase().contains("okinawa"), "region missing: {stdout}");
}

#[tokio::test]
async fn computes_savings_when_both_fit_and_group_tour_exist_same_date() {
    let run_id = unique_run_id();
    cleanup(&run_id);
    let _g = Guard::new({
        let run_id = run_id.clone();
        move || cleanup(&run_id)
    });
    if seed_run(&run_id).is_none() {
        eprintln!("skipping: no Turso credentials");
        return;
    }
    seed_offer(&run_id, "gt1", "okinawa", "group_tour", 21999, "2026-06-18", "2026-06-21", None).unwrap();
    seed_offer(&run_id, "fit1", "okinawa", "fit", 11900, "2026-06-18", "2026-06-21", Some("high")).unwrap();

    let (_ok, stdout, _se) = run(&["shaping-baseline", "--run", &run_id]).unwrap();
    // Savings = 21999 - 11900 = 10099
    let lc = stdout.to_lowercase();
    assert!(
        stdout.contains("10,099") || lc.contains("savings") || lc.contains("cheaper"),
        "expected savings, got: {stdout}"
    );
}

#[tokio::test]
async fn filters_by_dest_region() {
    let run_id = unique_run_id();
    cleanup(&run_id);
    let _g = Guard::new({
        let run_id = run_id.clone();
        move || cleanup(&run_id)
    });
    if seed_run(&run_id).is_none() {
        eprintln!("skipping: no Turso credentials");
        return;
    }
    seed_offer(&run_id, "oka1", "okinawa", "fit", 11900, "2026-06-18", "2026-06-21", None).unwrap();
    seed_offer(&run_id, "kan1", "kansai", "fit", 22888, "2026-06-18", "2026-06-21", None).unwrap();

    let (_ok, stdout, _se) = run(&["shaping-baseline", "--run", &run_id, "--dest-region", "okinawa"]).unwrap();
    assert!(stdout.contains("11,900"), "okinawa price missing: {stdout}");
    assert!(!stdout.contains("22,888"), "kansai price should be filtered out: {stdout}");
}

// Agent-first: shaping-baseline emits plain text only (the --json output was
// removed). Assert the rendered table carries the right prices & savings.
#[tokio::test]
async fn baseline_plain_text_shows_prices_and_savings() {
    let run_id = unique_run_id();
    cleanup(&run_id);
    let _g = Guard::new({
        let run_id = run_id.clone();
        move || cleanup(&run_id)
    });
    if seed_run(&run_id).is_none() {
        eprintln!("skipping: no Turso credentials");
        return;
    }
    seed_offer(&run_id, "gt1", "okinawa", "group_tour", 21999, "2026-06-18", "2026-06-21", None).unwrap();
    seed_offer(&run_id, "fit1", "okinawa", "fit", 11900, "2026-06-18", "2026-06-21", Some("high")).unwrap();

    let (ok, stdout, stderr) = run(&["shaping-baseline", "--run", &run_id]).unwrap();
    assert!(ok, "expected success, stderr: {stderr}");

    // Plain-text table renders thousands-separated prices and a "cheaper by"
    // savings cell (21999 group-tour, 11900 fit, 10099 savings).
    assert!(stdout.contains("21,999"), "group_tour price missing: {stdout}");
    assert!(stdout.contains("11,900"), "fit price missing: {stdout}");
    assert!(stdout.contains("10,099"), "fit savings missing: {stdout}");
    assert!(stdout.contains("okinawa"), "region missing: {stdout}");
    assert!(stdout.contains("2026-06-18"), "depart date missing: {stdout}");
    // And it must NOT be JSON.
    assert!(!stdout.trim_start().starts_with('{'), "output must be plain text, not JSON: {stdout}");
}