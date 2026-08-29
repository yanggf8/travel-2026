//! Cluster A behavior lock: `ota show-capture`, write-offers under-extraction WARN +
//! next-step hints, and `db query-offers --capture-id/--job-id/--attempt-id`.
//! Real-Turso; skips cleanly if creds absent. Captures/offers are not plan-keyed.

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Mutex;

mod common;
use common::{bin, db_exec, db_exec_teardown, is_credless, nanos, teardown_offers, Guard};

static CLUSTER_A_LOCK: Mutex<()> = Mutex::new(());

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

fn lock() -> std::sync::MutexGuard<'static, ()> {
    CLUSTER_A_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

#[test]
fn show_capture_help_prints_usage() {
    for sub in ["show-capture", "show_capture"] {
        for flag in ["--help", "-h"] {
            let (ok, stdout, stderr) = run(&["ota", sub, flag]);
            let combined = format!("{stdout}{stderr}");
            assert!(ok, "{sub} {flag} should exit 0; stderr={stderr}");
            assert!(
                combined.contains("Usage"),
                "{sub} {flag} should print Usage; got: {combined}"
            );
        }
    }
}

#[test]
fn show_capture_missing_id_fails_loud() {
    let (ok, stdout, stderr) = run(&["ota", "show-capture"]);
    assert!(!ok, "missing capture_id must exit 1");
    let combined = format!("{stdout}{stderr}");
    assert!(
        combined.contains("Usage"),
        "missing capture_id must print Usage; got: {combined}"
    );
}

#[test]
fn unknown_ota_subcommand_lists_show_capture() {
    let (ok, _stdout, stderr) = run(&["ota", "not-a-real-sub"]);
    assert!(!ok);
    assert!(
        stderr.contains("show-capture"),
        "unknown-subcommand Usage must list show-capture; stderr={stderr}"
    );
}

#[test]
fn show_capture_unknown_flag_fails_loud() {
    let (ok, _stdout, stderr) = run(&["ota", "show-capture", "--bogus"]);
    assert!(!ok);
    assert!(
        stderr.contains("unknown flag") || stderr.contains("Usage"),
        "unknown flag must fail loud; stderr={stderr}"
    );
}

fn teardown_capture_rows(capture_id: &str, offer_id: Option<&str>, dest: Option<&str>) {
    if let Some(id) = offer_id {
        teardown_offers(&[id]);
    }
    let _ = db_exec_teardown(&format!(
        "DELETE FROM captures WHERE capture_id = '{capture_id}'"
    ));
    if let Some(slug) = dest {
        let _ = db_exec_teardown(&format!(
            "DELETE FROM destination_config WHERE slug = '{slug}'"
        ));
    }
}

#[tokio::test]
async fn show_capture_dumps_raw_text_verbatim() {
    let _lock = lock();
    let (ok, _o, err) = run(&["db", "migrate"]);
    if !ok && is_credless(&err) {
        eprintln!("skipping (no creds): {}", err.trim());
        return;
    }

    let suffix = nanos();
    let capture_id = format!("test-cap-{suffix}");
    let now = "2026-08-26T00:00:00Z";
    let raw = "LINE-ONE of capture / LINE-TWO with 中文 and $99";
    teardown_capture_rows(&capture_id, None, None);
    let _g = Guard::new({
        let capture_id = capture_id.clone();
        move || teardown_capture_rows(&capture_id, None, None)
    });

    let sql = format!(
        "INSERT INTO captures (capture_id, source_id, url, captured_at, raw_text) \
         VALUES ('{capture_id}', 'zzshow', 'https://example.test/page', '{now}', '{raw}')"
    );
    let Some(_) = db_exec(&sql) else {
        eprintln!("skipping (no creds mid-test)");
        return;
    };

    let (ok, stdout, stderr) = run(&["ota", "show-capture", &capture_id]);
    assert!(ok, "show-capture failed: stderr={stderr}");
    assert_eq!(stdout, raw, "raw_text must be dumped verbatim to stdout");
    assert!(
        stderr.contains("source_id=zzshow"),
        "stderr summary must include source_id; stderr={stderr}"
    );
    assert!(
        stderr.contains("url=https://example.test/page"),
        "stderr summary must include url; stderr={stderr}"
    );
    assert!(
        stderr.contains(&format!("captured_at={now}")),
        "stderr summary must include captured_at; stderr={stderr}"
    );

    let (ok, stdout, stderr) = run(&["ota", "show_capture", &capture_id]);
    assert!(ok, "show_capture alias failed: stderr={stderr}");
    assert_eq!(stdout, raw, "alias must dump the same raw_text");

    let (ok, _stdout, stderr) = run(&["ota", "show-capture", "test-missing-capture-id"]);
    assert!(!ok, "missing capture must exit 1");
    assert!(
        stderr.contains("not found"),
        "missing capture must fail loud; stderr={stderr}"
    );
}

fn empty_tsv(suffix: u128) -> PathBuf {
    let p = std::env::temp_dir().join(format!("zz_ota_empty_{suffix}.tsv"));
    fs::write(&p, "type\tprice_per_person\n").expect("write empty tsv");
    p
}

fn teardown_write(
    job_id: &str,
    capture_id: &str,
    source_id: &str,
    dest: &str,
    tsv: Option<&PathBuf>,
) {
    let _ = db_exec_teardown(&format!(
        "DELETE FROM offers WHERE produced_by_job_id='{job_id}'"
    ));
    let _ = db_exec_teardown(&format!("DELETE FROM ota_attempts WHERE job_id='{job_id}'"));
    let _ = db_exec_teardown(&format!("DELETE FROM ota_job_params WHERE job_id='{job_id}'"));
    let _ = db_exec_teardown(&format!("DELETE FROM ota_jobs WHERE job_id='{job_id}'"));
    let _ = db_exec_teardown(&format!(
        "DELETE FROM captures WHERE capture_id='{capture_id}'"
    ));
    let _ = db_exec_teardown(&format!(
        "DELETE FROM ota_source_coverage WHERE source_id='{source_id}'"
    ));
    let _ = db_exec_teardown(&format!(
        "DELETE FROM ota_sources WHERE source_id='{source_id}'"
    ));
    let _ = db_exec_teardown(&format!(
        "DELETE FROM destination_config WHERE slug='{dest}'"
    ));
    if let Some(p) = tsv {
        let _ = fs::remove_file(p);
    }
}

#[tokio::test]
async fn write_offers_zero_candidates_warns_and_prints_next() {
    let _lock = lock();
    let (ok, _o, err) = run(&["db", "migrate"]);
    if !ok && is_credless(&err) {
        eprintln!("skipping (no creds): {}", err.trim());
        return;
    }

    let suffix = nanos();
    let job_id = format!("test-job-{suffix}");
    let capture_id = format!("test-cap-{suffix}");
    let source_id = format!("test-src-{suffix}");
    let dest = format!("test_dest_{suffix}");
    let token = format!("test-tok-{suffix}");
    let now = "2026-08-26T01:00:00Z";
    let tsv = empty_tsv(suffix);
    teardown_write(&job_id, &capture_id, &source_id, &dest, None);
    let _g = Guard::new({
        let (job_id, capture_id, source_id, dest, tsv) = (
            job_id.clone(),
            capture_id.clone(),
            source_id.clone(),
            dest.clone(),
            tsv.clone(),
        );
        move || teardown_write(&job_id, &capture_id, &source_id, &dest, Some(&tsv))
    });

    let seed = [
        format!(
            "INSERT OR IGNORE INTO destination_config \
             (slug, display_name, timezone, currency, origin) \
             VALUES ('{dest}', 'ZZ Cluster A', 'Asia/Tokyo', 'JPY', 'TPE')"
        ),
        format!(
            "INSERT OR IGNORE INTO ota_sources (source_id, name, status, updated_at) \
             VALUES ('{source_id}', 'ZZ Cluster A', 'active', '{now}')"
        ),
        format!(
            "INSERT OR IGNORE INTO ota_source_coverage \
             (source_id, product_type, proven, method, updated_at) \
             VALUES ('{source_id}', 'fit', 1, 'agent_parse', '{now}')"
        ),
        format!(
            "INSERT INTO captures (capture_id, source_id, url, captured_at, raw_text) \
             VALUES ('{capture_id}', '{source_id}', 'https://example.test/', '{now}', 'empty page')"
        ),
        format!(
            "INSERT INTO ota_jobs (job_id, source_id, product_type, status, claim_token, claimed_by, \
             claimed_at, heartbeat_at, lease_expires_at, created_at, updated_at) \
             VALUES ('{job_id}', '{source_id}', 'fit', 'claimed', '{token}', 'cluster-a', \
             '{now}', '{now}', '2026-08-26T02:00:00Z', '{now}', '{now}')"
        ),
    ];
    for sql in &seed {
        if db_exec(sql).is_none() {
            eprintln!("skipping (no creds mid-test)");
            return;
        }
    }

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
        &dest,
    ]);
    assert!(ok, "write-offers 0-candidate should succeed: stderr={stderr}");
    assert!(
        stdout.contains("candidates\t0"),
        "stdout table must keep candidates=0; stdout={stdout}"
    );
    assert!(
        stdout.contains("status\tsucceeded"),
        "status stays succeeded; stdout={stdout}"
    );
    assert!(
        stderr.contains("WARN: 0 candidates — page may have been empty or extraction missed content"),
        "must warn on 0 candidates; stderr={stderr}"
    );
    assert!(
        stderr.contains("Next: promote with `travel promote-offers --from-offers --dest"),
        "must print promote next-step; stderr={stderr}"
    );
}

#[tokio::test]
async fn db_query_offers_filters_by_capture_job_attempt() {
    let _lock = lock();
    let (ok, _o, err) = run(&["db", "migrate"]);
    if !ok && is_credless(&err) {
        eprintln!("skipping (no creds): {}", err.trim());
        return;
    }

    let suffix = nanos();
    let offer_id = format!("test-off-{suffix}");
    let capture_id = format!("test-cap-{suffix}");
    let job_id = format!("test-job-{suffix}");
    let attempt_id = format!("test-att-{suffix}");
    let scraped_at = "2026-08-26T03:00:00Z";
    teardown_offers(&[&offer_id]);
    let _g = Guard::new({
        let offer_id = offer_id.clone();
        move || teardown_offers(&[&offer_id])
    });

    let sql = format!(
        "INSERT INTO offers (id, source_id, type, name, price_per_person, currency, \
         destination, departure_date, scraped_at, capture_id, produced_by_job_id, \
         produced_by_attempt_id) \
         VALUES ('{offer_id}', 'zzcluster', 'package', 'Cluster A Hotel', 12345, 'TWD', \
         'zz_cluster_a', '2026-09-01', '{scraped_at}', '{capture_id}', '{job_id}', '{attempt_id}')"
    );
    if db_exec(&sql).is_none() {
        eprintln!("skipping (no creds mid-test)");
        return;
    }

    let (ok, stdout, stderr) = run(&[
        "db",
        "query-offers",
        "--capture-id",
        &capture_id,
        "--sql",
    ]);
    assert!(ok, "query-offers --capture-id failed: stderr={stderr}");
    assert!(
        stdout.contains("capture_id = ?1") || stdout.contains("capture_id"),
        "--sql must show capture_id predicate; stdout={stdout}"
    );
    assert!(
        stdout.contains("CAPTURE") && stdout.contains("JOB") && stdout.contains("ATTEMPT"),
        "table must include CAPTURE/JOB/ATTEMPT columns; stdout={stdout}"
    );
    assert!(
        stdout.contains("Cluster A Hotel") || stdout.contains(&offer_id) || stdout.contains("zzcluster"),
        "matching offer must appear; stdout={stdout}"
    );

    let (ok, stdout, stderr) = run(&["db", "query-offers", "--job-id", &job_id]);
    assert!(ok, "query-offers --job-id failed: stderr={stderr}");
    assert!(
        stdout.contains("zzcluster") || stdout.contains("Cluster A Hotel"),
        "--job-id must find the seeded offer; stdout={stdout}"
    );

    let (ok, stdout, stderr) = run(&["db", "query-offers", "--attempt-id", &attempt_id]);
    assert!(ok, "query-offers --attempt-id failed: stderr={stderr}");
    assert!(
        stdout.contains("zzcluster") || stdout.contains("Cluster A Hotel"),
        "--attempt-id must find the seeded offer; stdout={stdout}"
    );

    let (ok, stdout, _stderr) = run(&[
        "db",
        "query-offers",
        "--capture-id",
        "test-no-such-capture",
    ]);
    assert!(ok, "empty filter is still a successful read");
    assert!(
        stdout.contains("No offers found") || !stdout.contains("zzcluster"),
        "wrong capture_id must not return the seeded offer; stdout={stdout}"
    );

    let (ok, _stdout, stderr) = run(&["db", "query-offers", "--capture", "x"]);
    assert!(!ok, "unknown flag must fail loud");
    assert!(
        stderr.contains("unknown flag"),
        "unknown flag must mention unknown flag; stderr={stderr}"
    );
}

#[tokio::test]
async fn write_offers_reingest_same_tsv_dedupes_across_jobs() {
    let _lock = lock();
    let (ok, _o, err) = run(&["db", "migrate"]);
    if !ok && is_credless(&err) {
        eprintln!("skipping (no creds): {}", err.trim());
        return;
    }

    let suffix = nanos();
    let dest = format!("test_dest_{suffix}");
    let source_id = format!("test-src-{suffix}");
    let capture_id = format!("test-cap-{suffix}");
    let job1 = format!("test-job-a-{suffix}");
    let job2 = format!("test-job-b-{suffix}");
    let token1 = format!("test-tok-a-{suffix}");
    let token2 = format!("test-tok-b-{suffix}");
    let now = "2026-08-26T01:00:00Z";
    let lease = "2026-08-26T02:00:00Z";
    let tsv = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/zz_ota_offers.tsv");
    // Pre-clean + panic-safe teardown for BOTH jobs' rows plus the shared capture/source/dest.
    // Exact ids only — no prefix sweeps on the shared DB.
    let cleanup = {
        let (dest, source_id, capture_id, job1, job2) = (
            dest.clone(),
            source_id.clone(),
            capture_id.clone(),
            job1.clone(),
            job2.clone(),
        );
        move || {
            for job in [&job1, &job2] {
                let _ = db_exec_teardown(&format!(
                    "DELETE FROM offers WHERE produced_by_job_id='{job}'"
                ));
                let _ =
                    db_exec_teardown(&format!("DELETE FROM ota_attempts WHERE job_id='{job}'"));
                let _ = db_exec_teardown(&format!(
                    "DELETE FROM ota_job_params WHERE job_id='{job}'"
                ));
                let _ = db_exec_teardown(&format!("DELETE FROM ota_jobs WHERE job_id='{job}'"));
            }
            let _ = db_exec_teardown(&format!(
                "DELETE FROM captures WHERE capture_id='{capture_id}'"
            ));
            let _ = db_exec_teardown(&format!(
                "DELETE FROM ota_sources WHERE source_id='{source_id}'"
            ));
            let _ = db_exec_teardown(&format!("DELETE FROM destination_config WHERE slug='{dest}'"));
        }
    };
    cleanup();
    let _g = Guard::new(cleanup);

    let seed = [
        format!(
            "INSERT OR IGNORE INTO destination_config \
             (slug, display_name, timezone, currency, origin) \
             VALUES ('{dest}', 'ZZ Cluster A', 'Asia/Tokyo', 'JPY', 'TPE')"
        ),
        format!(
            "INSERT OR IGNORE INTO ota_sources (source_id, name, status, updated_at) \
             VALUES ('{source_id}', 'ZZ Cluster A', 'active', '{now}')"
        ),
        format!(
            "INSERT INTO captures (capture_id, source_id, url, captured_at, raw_text) \
             VALUES ('{capture_id}', '{source_id}', 'https://example.test/pkg', '{now}', 'package page')"
        ),
        // Job product_type must match the TSV offer kind (package), or write-offers rejects.
        format!(
            "INSERT INTO ota_jobs (job_id, source_id, product_type, status, claim_token, claimed_by, \
             claimed_at, heartbeat_at, lease_expires_at, created_at, updated_at) \
             VALUES ('{job1}', '{source_id}', 'package', 'claimed', '{token1}', 'cluster-a', \
             '{now}', '{now}', '{lease}', '{now}', '{now}')"
        ),
        format!(
            "INSERT INTO ota_jobs (job_id, source_id, product_type, status, claim_token, claimed_by, \
             claimed_at, heartbeat_at, lease_expires_at, created_at, updated_at) \
             VALUES ('{job2}', '{source_id}', 'package', 'claimed', '{token2}', 'cluster-a', \
             '{now}', '{now}', '{lease}', '{now}', '{now}')"
        ),
    ];
    for sql in &seed {
        if db_exec(sql).is_none() {
            eprintln!("skipping (no creds mid-test)");
            return;
        }
    }

    let write = |job: &str, token: &str| {
        run(&[
            "ota",
            "write-offers",
            job,
            "--capture",
            &capture_id,
            "--claim-token",
            token,
            "--tsv",
            tsv.to_str().unwrap(),
            "--dest",
            &dest,
        ])
    };

    // First ingest inserts.
    let (ok1, out1, err1) = write(&job1, &token1);
    assert!(ok1, "first write-offers should succeed: stderr={err1}");
    assert!(out1.contains("inserted\t1"), "first run inserts the offer; stdout={out1}");

    // Re-ingest the SAME TSV through a second job: content dedup, not duplication.
    let (ok2, out2, err2) = write(&job2, &token2);
    assert!(ok2, "re-ingest write-offers should succeed: stderr={err2}");
    assert!(out2.contains("inserted\t0"), "re-ingest inserts nothing; stdout={out2}");
    assert!(out2.contains("deduped\t1"), "re-ingest dedupes the candidate; stdout={out2}");

    // User-visible symptom check: the package appears exactly once after the re-ingest, and its
    // hotel shows in the NAME column (agent-path rows have NULL offers.name → COALESCE falls
    // through to hotel_name).
    let (okq, outq, errq) = run(&["db", "query-offers", "--capture-id", &capture_id]);
    assert!(okq, "query-offers failed: {errq}");
    assert!(outq.contains("Found 1 offer(s)"), "exactly one row survives; stdout={outq}");
    assert_eq!(
        outq.matches(&source_id).count(),
        1,
        "the package must appear exactly once after re-ingest; stdout={outq}"
    );
    assert_eq!(
        outq.matches("HOTEL ZZTEST").count(),
        1,
        "hotel_name must surface in the NAME column; stdout={outq}"
    );
}
