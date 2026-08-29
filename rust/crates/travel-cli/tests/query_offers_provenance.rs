//! Integration tests for query-offers provenance filters
//! (`--capture-id` / `--job-id` / `--attempt-id`).
//! Offers are not plan-keyed — teardown DELETEs the test offer id.

use std::process::Command;
use std::sync::Mutex;

mod common;
use common::{bin, db_exec, db_exec_teardown, is_credless, nanos, teardown_offers, Guard};

static QOP_LOCK: Mutex<()> = Mutex::new(());

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

#[tokio::test]
async fn query_offers_filters_by_capture_job_attempt() {
    let _lock = QOP_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (ok, _o, err) = run(&["db", "migrate"]);
    if !ok && is_credless(&err) {
        eprintln!("skipping (no creds): {}", err.trim());
        return;
    }

    let suffix = nanos();
    let offer_id = format!("test-qop-{suffix}");
    let capture_id = format!("test-qop-cap-{suffix}");
    let job_id = format!("test-qop-job-{suffix}");
    let attempt_id = format!("test-qop-att-{suffix}");
    let scraped = "2026-08-26T11:00:00Z";

    let _ = db_exec_teardown(&format!("DELETE FROM offers WHERE id='{offer_id}'"));
    let _g = Guard::new({
        let offer_id = offer_id.clone();
        move || teardown_offers(&[&offer_id])
    });

    db_exec(&format!(
        "INSERT INTO offers \
         (id, source_id, type, name, price_per_person, currency, destination, \
          departure_date, return_date, nights, scraped_at, capture_id, \
          produced_by_job_id, produced_by_attempt_id) \
         VALUES ('{offer_id}', 'zzqop', 'package', 'QOP HOTEL', 12345, 'TWD', \
          'zz_qop_dest', '2026-09-01', '2026-09-05', 4, '{scraped}', \
          '{capture_id}', '{job_id}', '{attempt_id}')"
    ))
    .expect("seed offer");

    let (ok, stdout, stderr) = run(&[
        "db",
        "query-offers",
        "--capture-id",
        &capture_id,
        "--sql",
    ]);
    assert!(ok, "db query-offers --capture-id failed: {stderr}");
    assert!(
        stdout.contains("capture_id"),
        "--sql must project capture_id; stdout={stdout}"
    );
    assert!(
        stdout.contains("produced_by_job_id"),
        "--sql must project produced_by_job_id; stdout={stdout}"
    );
    assert!(
        stdout.contains(&capture_id) || stdout.contains("QOP HOTEL") || stdout.contains("zzqop"),
        "matching offer must appear; stdout={stdout}"
    );
    assert!(
        stdout.contains(&format!("capture={capture_id}"))
            || stdout.contains(&capture_id),
        "output must include capture_id; stdout={stdout}"
    );

    let (ok, stdout, stderr) = run(&["db", "query-offers", "--job-id", &job_id]);
    assert!(ok, "db query-offers --job-id failed: {stderr}");
    assert!(
        stdout.contains("zzqop") || stdout.contains("QOP HOTEL") || stdout.contains(&job_id),
        "job-id filter must return the seeded offer; stdout={stdout}"
    );

    let (ok, stdout, stderr) = run(&["db", "query-offers", "--attempt-id", &attempt_id]);
    assert!(ok, "db query-offers --attempt-id failed: {stderr}");
    assert!(
        stdout.contains("zzqop") || stdout.contains("QOP HOTEL") || stdout.contains(&attempt_id),
        "attempt-id filter must return the seeded offer; stdout={stdout}"
    );

    let (ok, stdout, stderr) = run(&[
        "db",
        "query-offers",
        "--capture-id",
        "test-qop-cap-no-such",
    ]);
    assert!(ok, "empty match is still success; stderr={stderr}");
    assert!(
        stdout.contains("No offers found"),
        "unknown capture_id must match nothing; stdout={stdout}"
    );
    assert!(
        stdout.contains("capture_id: test-qop-cap-no-such"),
        "empty result must list applied capture_id filter; stdout={stdout}"
    );

    let (ok, stdout, stderr) = run(&["query-offers", "--capture-id", &capture_id]);
    assert!(ok, "query-offers --capture-id failed: {stderr}");
    assert!(
        stdout.contains("zzqop") || stdout.contains("QOP HOTEL") || stdout.contains(&capture_id),
        "user-facing query-offers must honor --capture-id; stdout={stdout}"
    );

    let (ok, _stdout, stderr) = run(&["db", "query-offers", "--totally-bogus"]);
    assert!(!ok, "unknown flag must fail loud");
    assert!(
        stderr.contains("unknown flag"),
        "unknown flag must fail loud; stderr={stderr}"
    );
}
