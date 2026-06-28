//! Regression: select-offer must POPULATE the P4 (and P3) process_statuses row even when
//! that row does not already exist. The original `set_status` was a bare UPDATE that
//! no-ops on a missing row, so a plan whose status ladder lacks P4 would silently fail to
//! populate P4 after selecting a hotel offer. (GitHub issue #5, item 2.)
//!
//! Real-Turso integration test; skips cleanly if creds absent.

use std::process::Command;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

mod common;
use common::Guard;

static UPSERT_LOCK: Mutex<()> = Mutex::new(());

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

fn teardown(plan: &str, dest: &str) {
    for tbl in [
        "plan_offer_date_pricing",
        "plan_offer_hotels",
        "plan_offers",
        "plan_offer_selection",
        "hotels",
        "hotel_access_lines",
    ] {
        let _ = db_exec(&format!(
            "DELETE FROM {tbl} WHERE plan_id = '{plan}' AND destination = '{dest}'"
        ));
    }
    for tbl in [
        "process_statuses",
        "plan_events",
        "plan_event_data",
        "operation_runs",
        "plan_metadata",
        "plans",
    ] {
        let _ = db_exec(&format!("DELETE FROM {tbl} WHERE plan_id = '{plan}'"));
    }
}

#[tokio::test]
async fn select_offer_populates_missing_p4_status_row() {
    let _guard = UPSERT_LOCK.lock().unwrap_or_else(|p| p.into_inner());

    let (ok, _o, err) = db_exec("SELECT 1");
    if !ok && is_skip(&err) {
        eprintln!("skipping select-offer upsert test (no Turso creds): {}", err.trim());
        return;
    }

    let tag = nanos();
    let dest = format!("test_upsert_{tag}");
    let plan = format!("test-upsert-{tag}");
    let offer = format!("up-offer-{tag}");

    teardown(&plan, &dest);
    // Guard runs teardown on return AND on panic (so a failing assert can't leak rows).
    let _g = Guard::new({
        let (plan, dest) = (plan.clone(), dest.clone());
        move || teardown(&plan, &dest)
    });

    // Minimal plan: plans + plan_metadata.
    db_exec(&format!(
        "INSERT OR REPLACE INTO plans (plan_id, schema_version, version) VALUES ('{plan}', '4.2.0', 0)"
    ));
    db_exec(&format!(
        "INSERT OR REPLACE INTO plan_metadata (plan_id, schema_version, active_destination) \
         VALUES ('{plan}', '4.2.0', '{dest}')"
    ));
    // Status ladder DELIBERATELY missing P4 (and P3). Only P3_4=researched exists, so the
    // selection transition validates — but the P4 row is absent, exercising the upsert.
    db_exec(&format!(
        "INSERT OR REPLACE INTO process_statuses (plan_id, destination, process_id, status) \
         VALUES ('{plan}', '{dest}', 'process_3_4_packages', 'researched')"
    ));

    // A hotel offer in plan_offers + its date_pricing + hotel rows (what select-offer reads).
    db_exec(&format!(
        "INSERT OR REPLACE INTO plan_offers \
            (plan_id, destination, id, source_id, type, title, price_per_person, currency, scraped_at) \
         VALUES ('{plan}', '{dest}', '{offer}', 'eztravel', 'package', 'Upsert Test', 10000, 'TWD', '2026-06-28T00:00:00Z')"
    ));
    db_exec(&format!(
        "INSERT OR REPLACE INTO plan_offer_date_pricing \
            (plan_id, destination, offer_id, date, price, currency) \
         VALUES ('{plan}', '{dest}', '{offer}', '2026-09-04', 10000, 'TWD')"
    ));
    db_exec(&format!(
        "INSERT OR REPLACE INTO plan_offer_hotels \
            (plan_id, destination, offer_id, name) \
         VALUES ('{plan}', '{dest}', '{offer}', 'Upsert Test Hotel')"
    ));

    // Run select-offer.
    let out = Command::new(bin())
        .args(["select-offer", &offer, "2026-09-04", "--plan-id", &plan])
        .output()
        .expect("run select-offer");
    let so_ok = out.status.success();
    let so_out = String::from_utf8_lossy(&out.stdout).into_owned();
    let so_err = String::from_utf8_lossy(&out.stderr).into_owned();
    if !so_ok && is_skip(&so_err) {
        eprintln!("skipping (no creds mid-test): {}", so_err.trim());
        return; // _cleanup Drop tears down
    }
    assert!(so_ok, "select-offer should succeed; err={so_err}");

    // Output must be ACCURATE: this is a hotel-only offer (no flights), so the message must
    // mention accommodation but must NOT claim transportation was populated (issue #5 item 3).
    assert!(
        so_out.contains("accommodation"),
        "output should report P4 accommodation populated; got:\n{so_out}"
    );
    assert!(
        !so_out.contains("transportation"),
        "hotel-only offer must NOT claim P3 transportation populated; got:\n{so_out}"
    );

    // P4 row must now EXIST and be 'populated' (the upsert created it).
    let (_, p4, _) = db_exec(&format!(
        "SELECT status FROM process_statuses WHERE plan_id='{plan}' AND destination='{dest}' \
         AND process_id='process_4_accommodation'"
    ));
    assert_eq!(
        scalar(&p4).as_deref(),
        Some("populated"),
        "select-offer must create+populate the missing P4 row (issue #5 upsert fix); got {p4:?}"
    );

    // _cleanup Drop runs teardown here (and on any panic above).
}
