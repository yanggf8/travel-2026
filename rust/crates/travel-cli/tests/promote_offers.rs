//! Real-Turso integration test for `promote-offers` — the bridge from the global
//! `offers` table to the plan-scoped `plan_offers` family that `select-offer` consumes.
//!
//! Seeds global `offers` rows + a minimal plan, runs the binary, asserts the mapped
//! plan_offers/date_pricing/hotel rows, then drives `select-offer` end-to-end to prove
//! the bridge actually feeds the cascade. Skips cleanly if Turso creds are absent.
//!
//! Design + review: docs/superpowers/specs/2026-06-28-promote-offers-bridge-design.md

use std::process::Command;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

mod common;
use common::Guard;

static PROMOTE_LOCK: Mutex<()> = Mutex::new(());

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_travel")
}

fn nanos() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
}

fn db_exec(sql: &str) -> (bool, String, String) {
    let out = Command::new(bin())
        .args(["db", "exec", sql])
        .output()
        .expect("run db exec");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn is_skip(stderr: &str) -> bool {
    stderr.contains("turso auth login")
        || stderr.contains("Missing Turso")
        || stderr.contains("failed to connect to Turso")
}

fn scalar(stdout: &str) -> Option<String> {
    stdout
        .lines()
        .find_map(|l| l.split_once(':').map(|(_, v)| v.trim().to_string()))
}

fn column(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .filter_map(|l| l.split_once(':').map(|(_, v)| v.trim().to_string()))
        .filter(|v| !v.is_empty())
        .collect()
}

/// Insert one global `offers` row. price<0 / date empty means "NULL" (skip-NULL test).
#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_arguments)]
fn seed_offer(
    dest: &str,
    id: &str,
    source: &str,
    price: Option<i64>,
    date: Option<&str>,
    hotel: Option<&str>,
    flight_out: Option<&str>,
    flight_ret: Option<&str>,
    includes: &str,
    scraped_at: &str,
) {
    let price_sql = price.map(|p| p.to_string()).unwrap_or_else(|| "NULL".into());
    let date_sql = date.map(|d| format!("'{d}'")).unwrap_or_else(|| "NULL".into());
    let hotel_sql = hotel.map(|h| format!("'{h}'")).unwrap_or_else(|| "NULL".into());
    let fout_sql = flight_out.map(|f| format!("'{f}'")).unwrap_or_else(|| "NULL".into());
    let fret_sql = flight_ret.map(|f| format!("'{f}'")).unwrap_or_else(|| "NULL".into());
    db_exec(&format!(
        "INSERT OR REPLACE INTO offers \
         (id, source_id, type, name, price_per_person, currency, destination, \
          departure_date, return_date, nights, availability, hotel_name, hotel_area, \
          airline, flight_outbound, flight_return, includes, scraped_at, source_file) \
         VALUES ('{id}', '{source}', 'package', '', {price}, 'TWD', '{dest}', \
          {date}, NULL, 4, 'available', {hotel}, '', NULL, {fout}, {fret}, '{includes}', \
          '{scraped}', 'test:promote')",
        id = id,
        source = source,
        price = price_sql,
        dest = dest,
        date = date_sql,
        hotel = hotel_sql,
        fout = fout_sql,
        fret = fret_sql,
        includes = includes,
        scraped = scraped_at,
    ));
}

fn seed_plan(plan: &str, dest: &str) {
    db_exec(&format!(
        "INSERT OR REPLACE INTO plans (plan_id, schema_version, version) \
         VALUES ('{plan}', '4.2.0', 0)"
    ));
    db_exec(&format!(
        "INSERT OR REPLACE INTO plan_metadata (plan_id, schema_version, active_destination) \
         VALUES ('{plan}', '4.2.0', '{dest}')"
    ));
    // A real plan (shaping-adopt/seed) carries the full process_statuses ladder.
    // select-offer's set_status is a bare UPDATE (select_offer.rs:477) — it no-ops if
    // the P3/P4 row is absent — so the rows must pre-exist as 'pending' for select-offer
    // to flip them to 'populated'. promote-offers itself flips P3_4 to 'researched'.
    for proc in [
        "process_1_date_anchor",
        "process_2_destination",
        "process_3_transportation",
        "process_4_accommodation",
        "process_3_4_packages",
        "process_5_daily_itinerary",
    ] {
        db_exec(&format!(
            "INSERT OR REPLACE INTO process_statuses (plan_id, destination, process_id, status) \
             VALUES ('{plan}', '{dest}', '{proc}', 'pending')"
        ));
    }
}

fn teardown(plan: &str, dest: &str, ids: &[&str]) {
    common::teardown_offers(ids);
    common::teardown_plan(plan, dest);
}

fn run_promote(plan: &str, dest: &str, extra: &[&str]) -> (bool, String, String) {
    let mut args = vec!["promote-offers", "--from-offers", "--dest", dest, "--plan-id", plan];
    args.extend_from_slice(extra);
    let out = Command::new(bin()).args(&args).output().expect("run promote-offers");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[tokio::test]
async fn promote_offers_bridges_into_plan_and_feeds_select() {
    let _guard = PROMOTE_LOCK.lock().unwrap_or_else(|p| p.into_inner());

    let (ok, _o, err) = db_exec("SELECT 1");
    if !ok && is_skip(&err) {
        eprintln!("skipping promote-offers test (no Turso creds): {}", err.trim());
        return;
    }

    let tag = nanos();
    let dest = format!("test_promote_{tag}");
    let plan = format!("test-promote-{tag}");
    let hotel_id = format!("po-hotel-{tag}");
    let flight_id = format!("po-flight-{tag}");
    let null_id = format!("po-null-{tag}");
    let dup_id = format!("po-dup-{tag}");
    let ids: Vec<&str> = vec![&hotel_id, &flight_id, &null_id, &dup_id];

    teardown(&plan, &dest, &ids);
    // Guard runs teardown on return AND on panic (so a failing assert can't leak rows).
    let _g = Guard::new({
        let (plan, dest) = (plan.clone(), dest.clone());
        let owned_ids: Vec<String> = ids.iter().map(|s| s.to_string()).collect();
        move || {
            let ids: Vec<&str> = owned_ids.iter().map(String::as_str).collect();
            teardown(&plan, &dest, &ids);
        }
    });
    seed_plan(&plan, &dest);

    // 1. hotel-only offer (the real case: hotel + NULL flights). Non-empty delimited `includes`
    //    with mixed delimiters + a blank item — locks the split + skip-empty behavior.
    seed_offer(&dest, &hotel_id, "eztravel", Some(15498), Some("2026-09-04"),
               Some("Test Hotel Ginza"), None, None, "含早餐; 機場接送、; WiFi", "2026-06-26T00:00:00Z");
    // 2. offer WITH both flight columns (proves the flight branch).
    seed_offer(&dest, &flight_id, "google_flights", Some(9000), Some("2026-09-05"),
               Some("Test Hotel B"), Some("CI100"), Some("CI101"), "", "2026-06-26T00:00:00Z");
    // 3. NULL price → must be skipped.
    seed_offer(&dest, &null_id, "settour", None, Some("2026-09-06"),
               Some("Test Hotel C"), None, None, "", "2026-06-26T00:00:00Z");
    // 4. duplicate id with two snapshots → only the latest (higher price) promotes.
    seed_offer(&dest, &dup_id, "besttour", Some(20000), Some("2026-09-07"),
               Some("Old Snapshot"), None, None, "", "2026-06-25T00:00:00Z");
    seed_offer(&dest, &dup_id, "besttour", Some(22000), Some("2026-09-07"),
               Some("New Snapshot"), None, None, "", "2026-06-26T00:00:00Z");

    // --- 5. dry-run writes nothing ---
    let (ok, out, err) = run_promote(&plan, &dest, &["--dry-run"]);
    if !ok && is_skip(&err) {
        eprintln!("skipping (no creds mid-test): {}", err.trim());
        return; // _cleanup Drop tears down
    }
    assert!(ok, "dry-run should succeed; err={err} out={out}");
    let (_, cnt, _) = db_exec(&format!(
        "SELECT COUNT(*) FROM plan_offers WHERE plan_id='{plan}' AND destination='{dest}'"
    ));
    assert_eq!(scalar(&cnt).as_deref(), Some("0"), "dry-run must write nothing; out={out}");

    // --- 6. real run ---
    let (ok, out, err) = run_promote(&plan, &dest, &[]);
    assert!(ok, "promote should succeed; err={err} out={out}");

    // plan_offers: hotel_id, flight_id, dup_id promoted; null_id skipped → 3 rows.
    let (_, ids_out, _) = db_exec(&format!(
        "SELECT id FROM plan_offers WHERE plan_id='{plan}' AND destination='{dest}' ORDER BY id"
    ));
    let got = column(&ids_out);
    assert!(got.iter().any(|i| i == &hotel_id), "hotel offer promoted; got {got:?}");
    assert!(got.iter().any(|i| i == &flight_id), "flight offer promoted; got {got:?}");
    assert!(got.iter().any(|i| i == &dup_id), "dup offer promoted; got {got:?}");
    assert!(!got.iter().any(|i| i == &null_id), "NULL-price offer skipped; got {got:?}");
    assert_eq!(got.len(), 3, "exactly 3 promoted; got {got:?}");

    // latest snapshot wins: dup offer title is the NEW snapshot.
    let (_, title, _) = db_exec(&format!(
        "SELECT title FROM plan_offers WHERE plan_id='{plan}' AND destination='{dest}' AND id='{dup_id}'"
    ));
    assert_eq!(scalar(&title).as_deref(), Some("New Snapshot"), "latest snapshot promoted");

    // date_pricing: hotel offer has one row at its departure_date with its price.
    let (_, dp, _) = db_exec(&format!(
        "SELECT price FROM plan_offer_date_pricing WHERE plan_id='{plan}' AND destination='{dest}' \
         AND offer_id='{hotel_id}' AND date='2026-09-04'"
    ));
    assert_eq!(scalar(&dp).as_deref(), Some("15498"), "date_pricing synthesized; out={dp}");

    // hotel row present.
    let (_, hn, _) = db_exec(&format!(
        "SELECT name FROM plan_offer_hotels WHERE plan_id='{plan}' AND destination='{dest}' AND offer_id='{hotel_id}'"
    ));
    assert_eq!(scalar(&hn).as_deref(), Some("Test Hotel Ginza"), "hotel mapped");

    // flights: ONLY the flight_id offer has plan_offer_flights rows (2: outbound+return).
    let (_, fn_hotel, _) = db_exec(&format!(
        "SELECT COUNT(*) FROM plan_offer_flights WHERE plan_id='{plan}' AND destination='{dest}' AND offer_id='{hotel_id}'"
    ));
    assert_eq!(scalar(&fn_hotel).as_deref(), Some("0"), "hotel-only offer has no flight rows");
    let (_, fn_flight, _) = db_exec(&format!(
        "SELECT COUNT(*) FROM plan_offer_flights WHERE plan_id='{plan}' AND destination='{dest}' AND offer_id='{flight_id}'"
    ));
    assert_eq!(scalar(&fn_flight).as_deref(), Some("2"), "flight offer has outbound+return rows");

    // plan_offer_includes: the hotel offer's `含早餐; 機場接送、; WiFi` splits on ; and 、, trims,
    // and SKIPS the blank item → exactly 3 ordered rows. Locks the split/skip-empty behavior.
    let (_, inc, _) = db_exec(&format!(
        "SELECT sort_order || ':' || item AS li FROM plan_offer_includes \
         WHERE plan_id='{plan}' AND destination='{dest}' AND offer_id='{hotel_id}' ORDER BY sort_order"
    ));
    assert_eq!(
        column(&inc),
        vec!["0:含早餐", "1:機場接送", "2:WiFi"],
        "includes split + skip-empty, ordered by sort_order; out={inc}"
    );
    let (_, inc_flight, _) = db_exec(&format!(
        "SELECT COUNT(*) FROM plan_offer_includes WHERE plan_id='{plan}' AND destination='{dest}' AND offer_id='{flight_id}'"
    ));
    assert_eq!(scalar(&inc_flight).as_deref(), Some("0"), "empty-includes offer writes no includes rows");

    // P3_4 status = researched.
    let (_, p34, _) = db_exec(&format!(
        "SELECT status FROM process_statuses WHERE plan_id='{plan}' AND destination='{dest}' AND process_id='process_3_4_packages'"
    ));
    assert_eq!(scalar(&p34).as_deref(), Some("researched"), "P3_4 → researched");

    // operation_runs: exactly one promote-offers row; plans.version bumped to 1.
    let (_, opn, _) = db_exec(&format!(
        "SELECT COUNT(*) FROM operation_runs WHERE plan_id='{plan}' AND command_type='promote-offers'"
    ));
    assert_eq!(scalar(&opn).as_deref(), Some("1"), "one promote-offers operation_run");
    let (_, ver, _) = db_exec(&format!("SELECT version FROM plans WHERE plan_id='{plan}'"));
    assert_eq!(scalar(&ver).as_deref(), Some("1"), "plans.version bumped by 1");

    // --- 7. re-promote idempotency: no dup rows, P3_4 stays researched ---
    let (ok, out, err) = run_promote(&plan, &dest, &[]);
    assert!(ok, "re-promote should succeed; err={err} out={out}");
    let (_, cnt2, _) = db_exec(&format!(
        "SELECT COUNT(*) FROM plan_offers WHERE plan_id='{plan}' AND destination='{dest}'"
    ));
    assert_eq!(scalar(&cnt2).as_deref(), Some("3"), "re-promote: still 3 rows (merge-by-id)");
    let (_, p34b, _) = db_exec(&format!(
        "SELECT status FROM process_statuses WHERE plan_id='{plan}' AND destination='{dest}' AND process_id='process_3_4_packages'"
    ));
    assert_eq!(scalar(&p34b).as_deref(), Some("researched"), "P3_4 still researched after re-promote");

    // --- 8. end-to-end: select-offer feeds off the bridged rows ---
    //   hotel offer → P4 populated, P3 NOT populated (no flights).
    let out = Command::new(bin())
        .args(["select-offer", &hotel_id, "2026-09-04", "--plan-id", &plan])
        .output()
        .expect("run select-offer");
    let so_ok = out.status.success();
    let so_err = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(so_ok, "select-offer on bridged hotel offer should succeed; err={so_err}");
    let (_, p4, _) = db_exec(&format!(
        "SELECT status FROM process_statuses WHERE plan_id='{plan}' AND destination='{dest}' AND process_id='process_4_accommodation'"
    ));
    assert_eq!(scalar(&p4).as_deref(), Some("populated"), "select-offer populated P4 from bridged hotel");
    let (_, p3, _) = db_exec(&format!(
        "SELECT status FROM process_statuses WHERE plan_id='{plan}' AND destination='{dest}' AND process_id='process_3_transportation'"
    ));
    // P3 must NOT be 'populated' for a hotel-only offer (no flights to populate).
    assert_ne!(scalar(&p3).as_deref(), Some("populated"), "hotel-only offer must not populate P3");

    // _cleanup Drop runs teardown here (and on any panic above).
}
