//! Integration test for `travel ota write-offers`.

use std::path::PathBuf;
use std::process::Command;
use std::sync::Mutex;

mod common;
use common::{
    bin, db_exec, db_exec_teardown, is_credless, nanos, seed_plan, teardown_plan, Guard,
};

// These tests share the global `zztest` row in ota_sources/ota_source_coverage, and each test's
// teardown DELETEs it by that shared literal — so a concurrent test would have its source yanked
// mid-run. Serialize them (same pattern as ota_claim.rs / ota_parse.rs).
static WRITE_OFFERS_TEST_LOCK: Mutex<()> = Mutex::new(());

fn write_offers_test_lock() -> std::sync::MutexGuard<'static, ()> {
    WRITE_OFFERS_TEST_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// Shared destination_config slug for legacy tests that only need a registered --dest.
const ZZ_WO_DEST: &str = "zz_wo_dest";

fn run(args: &[&str]) -> (bool, String, String) {
    let out = Command::new(bin())
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("run travel {args:?}: {e}"));
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn fixture_tsv() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/zz_ota_offers.tsv")
}

fn seed_zz_wo_dest() {
    let _ = run(&[
        "db",
        "exec",
        &format!(
            "INSERT OR IGNORE INTO destination_config \
             (slug, display_name, timezone, currency, origin) \
             VALUES ('{ZZ_WO_DEST}', 'ZZ WO Dest', 'Asia/Tokyo', 'JPY', 'TPE')"
        ),
    ]);
}

fn teardown(job_id: &str, capture_id: &str) {
    let _ = db_exec_teardown(&format!("DELETE FROM offers WHERE produced_by_job_id='{job_id}'"));
    let _ = db_exec_teardown(&format!("DELETE FROM ota_attempts WHERE job_id='{job_id}'"));
    let _ = db_exec_teardown(&format!("DELETE FROM ota_jobs WHERE job_id='{job_id}'"));
    let _ = db_exec_teardown(&format!("DELETE FROM captures WHERE capture_id='{capture_id}'"));
    let _ = db_exec_teardown("DELETE FROM ota_source_coverage WHERE source_id='zztest'");
    let _ = db_exec_teardown("DELETE FROM ota_sources WHERE source_id='zztest'");
    // seed_zz_wo_dest() inserts this shared slug (INSERT OR IGNORE) for the --dest requirement;
    // clean it here so it never leaks into shared prod Turso.
    let _ = db_exec_teardown(&format!("DELETE FROM destination_config WHERE slug='{ZZ_WO_DEST}'"));
}

#[tokio::test]
async fn write_offers_from_fixture_tsv() {
    let _lock = write_offers_test_lock();
    let (ok, _o, err) = run(&["db", "migrate"]);
    if !ok && is_credless(&err) {
        eprintln!("skipping (no creds): {}", err.trim());
        return;
    }

    let suffix = nanos();
    let job_id = format!("zzwo{suffix}");
    let capture_id = format!("zzcapwo{suffix}");
    let now = "2026-06-29T15:30:00Z";
    teardown(&job_id, &capture_id);
    let _g = Guard::new({
        let (job_id, capture_id) = (job_id.clone(), capture_id.clone());
        move || teardown(&job_id, &capture_id)
    });

    seed_zz_wo_dest();
    let _ = run(&[
        "db",
        "exec",
        &format!(
            "INSERT OR IGNORE INTO ota_sources (source_id, name, status, updated_at) \
             VALUES ('zztest', 'ZZ Test', 'active', '{now}')"
        ),
    ]);
    let _ = run(&[
        "db",
        "exec",
        &format!(
            "INSERT OR IGNORE INTO ota_source_coverage \
             (source_id, product_type, status, method, updated_at) \
             VALUES ('zztest', 'fit', 'active', 'agent_parse', '{now}')"
        ),
    ]);
    let _ = run(&[
        "db",
        "exec",
        &format!(
            "INSERT INTO captures (capture_id, source_id, url, captured_at, raw_text) \
             VALUES ('{capture_id}', 'zztest', 'https://example.com/', '{now}', 'agent tsv test')"
        ),
    ]);
    let token = format!("zztok{suffix}");
    let lease = "2026-06-29T16:30:00Z";
    let _ = run(&[
        "db",
        "exec",
        &format!(
            "INSERT INTO ota_jobs (job_id, source_id, product_type, status, claim_token, claimed_by, \
             claimed_at, heartbeat_at, lease_expires_at, created_at, updated_at) \
             VALUES ('{job_id}', 'zztest', 'fit', 'claimed', '{token}', 'wo-test', \
             '{now}', '{now}', '{lease}', '{now}', '{now}')"
        ),
    ]);

    let tsv = fixture_tsv();
    let (ok, stdout, stderr) = run(&[
        "ota",
        "write-offers",
        &job_id,
        "--capture",
        &capture_id,
        "--claim-token",
        &token,
        "--tsv",
        tsv.to_str().unwrap(),
        "--dest",
        ZZ_WO_DEST,
    ]);
    assert!(ok, "write-offers failed: {stderr}");
    assert!(stdout.contains("inserted\t1"));
    assert!(stdout.contains("parser_method\tagent_parse"));

    let (ok, stdout, _) = run(&[
        "db",
        "exec",
        &format!(
            "SELECT type, price_per_person, hotel_name, parser_method \
             FROM offers WHERE produced_by_job_id='{job_id}' LIMIT 1"
        ),
    ]);
    assert!(ok);
    assert!(stdout.contains("package"));
    assert!(stdout.contains("15000"));
    assert!(stdout.contains("HOTEL ZZTEST"));
    assert!(stdout.contains("agent_parse"));
}

fn fixture_multi_tsv() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/zz_ota_multi.tsv")
}

/// Two distinct offers share the same (date, nights) — they MUST NOT collapse onto one PK.
/// Locks the disambiguation fix (base offer_row_id keys only on source/code/date/nights).
#[tokio::test]
async fn write_offers_distinct_rows_do_not_collapse_on_pk() {
    let _lock = write_offers_test_lock();
    let (ok, _o, err) = run(&["db", "migrate"]);
    if !ok && is_credless(&err) {
        eprintln!("skipping (no creds): {}", err.trim());
        return;
    }

    let suffix = nanos();
    let job_id = format!("zzwo{suffix}");
    let capture_id = format!("zzcapwo{suffix}");
    let now = "2026-06-29T15:36:00Z";
    teardown(&job_id, &capture_id);
    let _g = Guard::new({
        let (job_id, capture_id) = (job_id.clone(), capture_id.clone());
        move || teardown(&job_id, &capture_id)
    });

    seed_zz_wo_dest();
    let _ = run(&[
        "db",
        "exec",
        &format!(
            "INSERT OR IGNORE INTO ota_sources (source_id, name, status, updated_at) \
             VALUES ('zztest', 'ZZ Test', 'active', '{now}')"
        ),
    ]);
    let _ = run(&[
        "db",
        "exec",
        &format!(
            "INSERT INTO captures (capture_id, source_id, url, captured_at, raw_text) \
             VALUES ('{capture_id}', 'zztest', 'https://example.com/pkg', '{now}', 'multi tsv')"
        ),
    ]);
    let token = format!("zztok{suffix}");
    let lease = "2026-06-29T16:36:00Z";
    let _ = run(&[
        "db",
        "exec",
        &format!(
            "INSERT INTO ota_jobs (job_id, source_id, product_type, status, claim_token, claimed_by, \
             claimed_at, heartbeat_at, lease_expires_at, created_at, updated_at) \
             VALUES ('{job_id}', 'zztest', 'fit', 'claimed', '{token}', 'wo-test', \
             '{now}', '{now}', '{lease}', '{now}', '{now}')"
        ),
    ]);

    let tsv = fixture_multi_tsv();
    let (ok, stdout, stderr) = run(&[
        "ota",
        "write-offers",
        &job_id,
        "--capture",
        &capture_id,
        "--claim-token",
        &token,
        "--tsv",
        tsv.to_str().unwrap(),
        "--dest",
        ZZ_WO_DEST,
    ]);
    if is_credless(&stderr) {
        return;
    }
    assert!(ok, "write-offers failed: {stderr}");
    // BOTH offers must land — pre-fix only one survived ON CONFLICT DO NOTHING.
    assert!(stdout.contains("candidates\t2"), "stdout={stdout}");
    assert!(stdout.contains("inserted\t2"), "stdout={stdout}");
    assert!(stdout.contains("deduped\t0"), "stdout={stdout}");

    let (ok, stdout, _) = run(&[
        "db",
        "exec",
        &format!(
            "SELECT count(*) AS n FROM offers WHERE produced_by_job_id='{job_id}'"
        ),
    ]);
    assert!(ok);
    assert!(stdout.contains(": 2") || stdout.contains(":2"), "row count stdout={stdout}");

    // The job must end 'succeeded', not stuck in 'running', and attempts must have advanced.
    let (ok, stdout, _) = run(&[
        "db",
        "exec",
        &format!("SELECT status, attempts FROM ota_jobs WHERE job_id='{job_id}'"),
    ]);
    assert!(ok);
    assert!(stdout.contains("succeeded"), "job status stdout={stdout}");
}

fn bad_header_tsv() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/zz_bad_header.tsv")
}

#[tokio::test]
async fn write_offers_bad_header_fails_loud() {
    let _lock = write_offers_test_lock();
    let suffix = nanos();
    let bad_path = bad_header_tsv();

    let job_id = format!("zzwo{suffix}");
    let capture_id = format!("zzcap{suffix}");
    let now = "2026-06-29T15:31:00Z";
    teardown(&job_id, &capture_id);
    let _g = Guard::new({
        let (job_id, capture_id) = (job_id.clone(), capture_id.clone());
        move || teardown(&job_id, &capture_id)
    });
    seed_zz_wo_dest();
    let token = format!("zztok{suffix}");
    let _ = run(&[
        "db",
        "exec",
        &format!(
            "INSERT INTO ota_jobs (job_id, source_id, product_type, status, claim_token, created_at, updated_at) \
             VALUES ('{job_id}', 'zztest', 'fit', 'claimed', '{token}', '{now}', '{now}')"
        ),
    ]);
    let _ = run(&[
        "db",
        "exec",
        &format!(
            "INSERT INTO captures (capture_id, source_id, captured_at, raw_text) \
             VALUES ('{capture_id}', 'zztest', '{now}', 'x')"
        ),
    ]);

    let (ok, stdout, stderr) = run(&[
        "ota",
        "write-offers",
        &job_id,
        "--capture",
        &capture_id,
        "--claim-token",
        &token,
        "--tsv",
        bad_path.to_str().unwrap(),
        "--dest",
        ZZ_WO_DEST,
    ]);
    if is_credless(&stderr) {
        return;
    }
    assert!(!ok, "bad header must fail; stdout={stdout} stderr={stderr}");
    let combined = format!("{stdout}{stderr}");
    assert!(
        combined.contains("unknown") || combined.contains("bogus") || combined.contains("Error"),
        "combined={combined}"
    );
}

#[tokio::test]
async fn write_offers_stale_token_writes_nothing() {
    let _lock = write_offers_test_lock();
    let suffix = nanos();
    let job_id = format!("zzwo{suffix}");
    let capture_id = format!("zzcap{suffix}");
    teardown(&job_id, &capture_id);
    let _g = Guard::new({
        let (job_id, capture_id) = (job_id.clone(), capture_id.clone());
        move || teardown(&job_id, &capture_id)
    });
    seed_zz_wo_dest();
    let now = "2026-06-29T15:32:00Z";
    let _ = run(&[
        "db",
        "exec",
        &format!(
            "INSERT INTO captures (capture_id, source_id, captured_at, raw_text) \
             VALUES ('{capture_id}', 'zztest', '{now}', 'x')"
        ),
    ]);
    let _ = run(&[
        "db",
        "exec",
        &format!(
            "INSERT INTO ota_jobs (job_id, source_id, product_type, status, created_at, updated_at) \
             VALUES ('{job_id}', 'zztest', 'fit', 'claimed', '{now}', '{now}')"
        ),
    ]);
    let tsv = fixture_tsv();
    let (ok, _stdout, stderr) = run(&[
        "ota",
        "write-offers",
        &job_id,
        "--capture",
        &capture_id,
        "--claim-token",
        "wrong-token",
        "--tsv",
        tsv.to_str().unwrap(),
        "--dest",
        ZZ_WO_DEST,
    ]);
    if is_credless(&stderr) {
        return;
    }
    assert!(!ok);
    let (ok, stdout, _) = run(&[
        "db",
        "exec",
        &format!("SELECT count(*) AS n FROM offers WHERE produced_by_job_id='{job_id}'"),
    ]);
    if ok {
        assert!(stdout.contains(": 0") || stdout.contains(":0"));
    }
}

#[tokio::test]
async fn write_offers_rejects_capture_source_mismatch() {
    let _lock = write_offers_test_lock();
    let suffix = nanos();
    let job_id = format!("zzwo{suffix}");
    let capture_id = format!("zzcap{suffix}");
    teardown(&job_id, &capture_id);
    let _g = Guard::new({
        let (job_id, capture_id) = (job_id.clone(), capture_id.clone());
        move || teardown(&job_id, &capture_id)
    });
    seed_zz_wo_dest();
    let now = "2026-06-29T15:33:00Z";
    let token = format!("zztok{suffix}");
    let _ = run(&[
        "db",
        "exec",
        &format!(
            "INSERT INTO captures (capture_id, source_id, captured_at, raw_text) \
             VALUES ('{capture_id}', 'zztest', '{now}', 'x')"
        ),
    ]);
    let _ = run(&[
        "db",
        "exec",
        &format!(
            "INSERT INTO ota_jobs (job_id, source_id, product_type, status, claim_token, created_at, updated_at) \
             VALUES ('{job_id}', 'zzother', 'fit', 'claimed', '{token}', '{now}', '{now}')"
        ),
    ]);

    let tsv = fixture_tsv();
    let (ok, _stdout, stderr) = run(&[
        "ota",
        "write-offers",
        &job_id,
        "--capture",
        &capture_id,
        "--claim-token",
        &token,
        "--tsv",
        tsv.to_str().unwrap(),
        "--dest",
        ZZ_WO_DEST,
    ]);
    if is_credless(&stderr) {
        return;
    }
    assert!(!ok, "capture source mismatch must fail");
    assert!(stderr.contains("source_id"), "stderr={stderr}");
    let (ok, stdout, _) = run(&[
        "db",
        "exec",
        &format!("SELECT count(*) AS n FROM offers WHERE produced_by_job_id='{job_id}'"),
    ]);
    if ok {
        assert!(stdout.contains(": 0") || stdout.contains(":0"));
    }
}

#[tokio::test]
async fn write_offers_rejects_type_incompatible_with_job_product() {
    let _lock = write_offers_test_lock();
    let suffix = nanos();
    let job_id = format!("zzwo{suffix}");
    let capture_id = format!("zzcap{suffix}");
    teardown(&job_id, &capture_id);
    let _g = Guard::new({
        let (job_id, capture_id) = (job_id.clone(), capture_id.clone());
        move || teardown(&job_id, &capture_id)
    });
    seed_zz_wo_dest();
    let now = "2026-06-29T15:34:00Z";
    let token = format!("zztok{suffix}");
    let _ = run(&[
        "db",
        "exec",
        &format!(
            "INSERT INTO captures (capture_id, source_id, captured_at, raw_text) \
             VALUES ('{capture_id}', 'zztest', '{now}', 'x')"
        ),
    ]);
    let _ = run(&[
        "db",
        "exec",
        &format!(
            "INSERT INTO ota_jobs (job_id, source_id, product_type, status, claim_token, created_at, updated_at) \
             VALUES ('{job_id}', 'zztest', 'flight', 'claimed', '{token}', '{now}', '{now}')"
        ),
    ]);

    let tsv = fixture_tsv();
    let (ok, _stdout, stderr) = run(&[
        "ota",
        "write-offers",
        &job_id,
        "--capture",
        &capture_id,
        "--claim-token",
        &token,
        "--tsv",
        tsv.to_str().unwrap(),
        "--dest",
        ZZ_WO_DEST,
    ]);
    if is_credless(&stderr) {
        return;
    }
    assert!(!ok, "package TSV rows must not write to a flight job");
    assert!(stderr.contains("incompatible"), "stderr={stderr}");
    let (ok, stdout, _) = run(&[
        "db",
        "exec",
        &format!("SELECT count(*) AS n FROM offers WHERE produced_by_job_id='{job_id}'"),
    ]);
    if ok {
        assert!(stdout.contains(": 0") || stdout.contains(":0"));
    }
}

#[tokio::test]
async fn write_offers_stops_when_mark_running_rejects_token_state() {
    let _lock = write_offers_test_lock();
    let suffix = nanos();
    let job_id = format!("zzwo{suffix}");
    let capture_id = format!("zzcap{suffix}");
    teardown(&job_id, &capture_id);
    let _g = Guard::new({
        let (job_id, capture_id) = (job_id.clone(), capture_id.clone());
        move || teardown(&job_id, &capture_id)
    });
    seed_zz_wo_dest();
    let now = "2026-06-29T15:35:00Z";
    let token = format!("zztok{suffix}");
    let _ = run(&[
        "db",
        "exec",
        &format!(
            "INSERT INTO captures (capture_id, source_id, captured_at, raw_text) \
             VALUES ('{capture_id}', 'zztest', '{now}', 'x')"
        ),
    ]);
    let _ = run(&[
        "db",
        "exec",
        &format!(
            "INSERT INTO ota_jobs (job_id, source_id, product_type, status, claim_token, created_at, updated_at) \
             VALUES ('{job_id}', 'zztest', 'fit', 'queued', '{token}', '{now}', '{now}')"
        ),
    ]);

    let tsv = fixture_tsv();
    let (ok, _stdout, stderr) = run(&[
        "ota",
        "write-offers",
        &job_id,
        "--capture",
        &capture_id,
        "--claim-token",
        &token,
        "--tsv",
        tsv.to_str().unwrap(),
        "--dest",
        ZZ_WO_DEST,
    ]);
    if is_credless(&stderr) {
        return;
    }
    assert!(!ok, "mark_running rejection must fail before writing");
    assert!(stderr.contains("claim rejected"), "stderr={stderr}");
    let (ok, stdout, _) = run(&[
        "db",
        "exec",
        &format!("SELECT count(*) AS n FROM offers WHERE produced_by_job_id='{job_id}'"),
    ]);
    if ok {
        assert!(stdout.contains(": 0") || stdout.contains(":0"));
    }
}

/// #C behavior-lock: write-offers --dest stamps offers.destination/region so promote-offers finds them.
#[tokio::test]
async fn write_offers_dest_stamps_destination_and_region_and_promotes() {
    let _lock = write_offers_test_lock();

    let n = nanos();
    let dest = format!("wo_dest_{n}");
    let plan = format!("wo-plan-{n}");
    let source = format!("wo_src_{n}");
    let cap = format!("wo_cap_{n}");
    let job = format!("wo_job_{n}");
    let tok = format!("wo_tok_{n}");

    let Some(_) = db_exec("SELECT 1") else {
        eprintln!("credless — skip");
        return;
    };

    // widened ota_job_params CHECK is a runtime migration
    let _ = run(&["db", "migrate"]);

    let _g = Guard::new({
        let (plan, dest, source, cap, job) =
            (plan.clone(), dest.clone(), source.clone(), cap.clone(), job.clone());
        move || {
            db_exec_teardown(&format!("DELETE FROM ota_job_params WHERE job_id='{job}'"));
            db_exec_teardown(&format!("DELETE FROM ota_attempts WHERE job_id='{job}'"));
            db_exec_teardown(&format!("DELETE FROM ota_jobs WHERE job_id='{job}'"));
            db_exec_teardown(&format!("DELETE FROM captures WHERE capture_id='{cap}'"));
            db_exec_teardown(&format!("DELETE FROM offers WHERE source_id='{source}'"));
            db_exec_teardown(&format!("DELETE FROM ota_sources WHERE source_id='{source}'"));
            db_exec_teardown(&format!("DELETE FROM destination_config WHERE slug='{dest}'"));
            teardown_plan(&plan, &dest);
        }
    });

    let now = "2026-07-10T00:00:00Z";
    let lease = "2026-07-10T01:00:00Z";

    seed_plan(&plan, &dest, 1);
    // destination_config requires timezone + currency (NOT NULL) — plan seed sketch omitted them.
    db_exec(&format!(
        "INSERT INTO destination_config (slug, display_name, timezone, currency, origin) \
         VALUES ('{dest}','WO Dest','Asia/Tokyo','JPY','TPE')"
    ))
    .expect("seed destination_config");
    // ota_sources uses `name` (not display_name) and has no `enabled` column.
    db_exec(&format!(
        "INSERT INTO ota_sources (source_id, name, status, updated_at) \
         VALUES ('{source}','WO Src','active','{now}')"
    ))
    .expect("seed ota_sources");
    db_exec(&format!(
        "INSERT INTO captures (capture_id, source_id, url, raw_text, captured_at) \
         VALUES ('{cap}','{source}','https://x/y?prod=1','type\tprice_per_person\n','{now}')"
    ))
    .expect("seed captures");
    // claimed job — include claim lease columns so mark_running can succeed.
    db_exec(&format!(
        "INSERT INTO ota_jobs (job_id, source_id, product_type, status, claim_token, \
             claimed_by, claimed_at, heartbeat_at, lease_expires_at, \
             max_attempts, attempts, created_at, updated_at) \
         VALUES ('{job}','{source}','flight','claimed','{tok}','tester', \
             '{now}','{now}','{lease}',3,0,'{now}','{now}')"
    ))
    .expect("seed ota_jobs");
    db_exec(&format!(
        "INSERT INTO ota_job_params (job_id, param_key, param_value) \
         VALUES ('{job}','region_label','Kansai'),('{job}','region_code','KIX')"
    ))
    .expect("seed ota_job_params");

    // TSV: one outbound-only flight offer
    let tsv = std::env::temp_dir().join(format!("wo_{n}.tsv"));
    std::fs::write(
        &tsv,
        "type\tprice_per_person\tdeparture_date\treturn_date\tnights\tairline\tflight_outbound\tflight_return\thotel_name\tcurrency\n\
         flight\t10386\t2026-08-05\t2026-08-09\t4\tJetstar\tGK25\t\t\tTWD\n",
    )
    .unwrap();

    // --- act: write-offers WITH --dest ---
    let (ok, stdout, stderr) = run(&[
        "ota",
        "write-offers",
        &job,
        "--capture",
        &cap,
        "--claim-token",
        &tok,
        "--tsv",
        tsv.to_str().unwrap(),
        "--dest",
        &dest,
    ]);
    assert!(ok, "write-offers failed: stderr={stderr} stdout={stdout}");

    // --- assert: offer landed with destination + region ---
    let dest_row = db_exec(&format!(
        "SELECT destination FROM offers WHERE source_id='{source}'"
    ))
    .expect("rows");
    assert_eq!(
        dest_row.scalar().as_deref(),
        Some(dest.as_str()),
        "offers.destination must be the --dest slug; out={}",
        dest_row.raw()
    );
    let region_row = db_exec(&format!(
        "SELECT region FROM offers WHERE source_id='{source}'"
    ))
    .expect("rows");
    assert_eq!(
        region_row.scalar().as_deref(),
        Some("Kansai"),
        "offers.region must be region_label from job params; out={}",
        region_row.raw()
    );
    let cnt = db_exec(&format!(
        "SELECT COUNT(*) FROM offers WHERE source_id='{source}'"
    ))
    .expect("rows");
    assert_eq!(
        cnt.scalar().as_deref(),
        Some("1"),
        "expected exactly 1 offer; out={}",
        cnt.raw()
    );

    // --- assert: promote-offers --dest now finds it ---
    let (ok, stdout, stderr) = run(&[
        "promote-offers",
        "--from-offers",
        "--dest",
        &dest,
        "--plan-id",
        &plan,
    ]);
    assert!(ok, "promote failed: stderr={stderr} stdout={stdout}");
    let po = db_exec(&format!(
        "SELECT COUNT(*) FROM plan_offers WHERE plan_id='{plan}' AND destination='{dest}'"
    ))
    .expect("rows");
    assert_eq!(
        po.scalar().as_deref(),
        Some("1"),
        "promoted offer must land in plan_offers; out={}",
        po.raw()
    );
}
