//! Integration tests for travel-db OTA repositories. Real-Turso; skips if creds absent.

use travel_db::repo::{captures, offers, ota_jobs};

mod common;
use common::{db_exec_teardown, is_credless, nanos, Guard};

fn exec_sql(sql: &str) {
    let _ = db_exec_teardown(sql);
}

async fn connect_write() -> Result<libsql::Connection, String> {
    let url = std::env::var("TRAVEL_TURSO_URL")
        .or_else(|_| std::env::var("TURSO_URL"))
        .map_err(|_| "TRAVEL_TURSO_URL not set".to_string())?;
    let token = std::env::var("TRAVEL_TURSO_WRITE_TOKEN")
        .or_else(|_| std::env::var("TURSO_TOKEN"))
        .map_err(|_| "TRAVEL_TURSO_WRITE_TOKEN not set".to_string())?;
    let db = libsql::Builder::new_remote(url, token)
        .build()
        .await
        .map_err(|e| format!("failed to connect to Turso: {e}"))?;
    db.connect()
        .map_err(|e| format!("failed to open Turso connection: {e}"))
}

async fn teardown_job(conn: &libsql::Connection, job_id: &str) {
    let _ = conn
        .execute(
            "DELETE FROM ota_observations WHERE job_id = ?1",
            libsql::params![job_id.to_string()],
        )
        .await;
    let _ = conn
        .execute(
            "DELETE FROM ota_attempts WHERE job_id = ?1",
            libsql::params![job_id.to_string()],
        )
        .await;
    let _ = conn
        .execute(
            "DELETE FROM ota_job_params WHERE job_id = ?1",
            libsql::params![job_id.to_string()],
        )
        .await;
    let _ = conn
        .execute(
            "DELETE FROM ota_jobs WHERE job_id = ?1",
            libsql::params![job_id.to_string()],
        )
        .await;
}

async fn teardown_offer(conn: &libsql::Connection, id: &str, scraped_at: &str) {
    let _ = conn
        .execute(
            "DELETE FROM offers WHERE id = ?1 AND scraped_at = ?2",
            libsql::params![id.to_string(), scraped_at.to_string()],
        )
        .await;
}

#[tokio::test]
async fn offer_insert_round_trips_provenance() {
    let conn = match connect_write().await {
        Ok(c) => c,
        Err(e) if is_credless(&e) => {
            eprintln!("skipping offer repo test: {e}");
            return;
        }
        Err(e) => panic!("connect failed: {e}"),
    };

    let offer_id = format!("zzrepo{}", nanos());
    let scraped_at = "2026-06-29T14:00:00Z";
    teardown_offer(&conn, &offer_id, scraped_at).await;
    let _g = Guard::new({
        let offer_id = offer_id.clone();
        let scraped_at = scraped_at.to_string();
        move || {
            exec_sql(&format!(
                "DELETE FROM offers WHERE id='{offer_id}' AND scraped_at='{scraped_at}'"
            ));
        }
    });

    let row = offers::OfferRow {
        id: offer_id.clone(),
        source_id: "zztest".to_string(),
        offer_type: "package".to_string(),
        scraped_at: scraped_at.to_string(),
        capture_id: Some("zzcap1".to_string()),
        produced_by_job_id: Some("zzjob1".to_string()),
        produced_by_attempt_id: Some("zzatt1".to_string()),
        parser_method: Some("regex".to_string()),
        capture_checksum: Some("abc123".to_string()),
        parser_rule_checksum: Some("def456".to_string()),
        normalizer_version: Some("norm-v1".to_string()),
        ..Default::default()
    };
    let result = offers::insert(&conn, &row).await.expect("insert");
    assert_eq!(result.inserted, 1);
    assert_eq!(result.deduped, 0);

    let got = offers::latest(&conn, &offer_id, scraped_at)
        .await
        .expect("latest")
        .expect("row");
    assert_eq!(got.capture_id.as_deref(), Some("zzcap1"));
    assert_eq!(got.produced_by_job_id.as_deref(), Some("zzjob1"));
    assert_eq!(got.parser_method.as_deref(), Some("regex"));
    assert_eq!(got.capture_checksum.as_deref(), Some("abc123"));

    let dup = offers::insert(&conn, &row).await.expect("dedup insert");
    assert_eq!(dup.inserted, 0);
    assert_eq!(dup.deduped, 1);
}

#[tokio::test]
async fn stale_claim_token_finish_affects_zero_rows() {
    let conn = match connect_write().await {
        Ok(c) => c,
        Err(e) if is_credless(&e) => {
            eprintln!("skipping stale token test: {e}");
            return;
        }
        Err(e) => panic!("connect failed: {e}"),
    };

    let job_id = format!("zzrepo{}", nanos());
    let now = "2026-06-29T14:01:00Z";
    teardown_job(&conn, &job_id).await;
    let _g = Guard::new({
        let job_id = job_id.clone();
        move || {
            exec_sql(&format!(
                "DELETE FROM ota_observations WHERE job_id='{job_id}'"
            ));
            exec_sql(&format!("DELETE FROM ota_attempts WHERE job_id='{job_id}'"));
            exec_sql(&format!("DELETE FROM ota_job_params WHERE job_id='{job_id}'"));
            exec_sql(&format!("DELETE FROM ota_jobs WHERE job_id='{job_id}'"));
        }
    });

    let token = format!("zztok{}", nanos());
    conn.execute(
        "INSERT INTO ota_jobs (job_id, source_id, product_type, status, claimed_by, claim_token, \
         claimed_at, heartbeat_at, lease_expires_at, created_at, updated_at) \
         VALUES (?1, 'zztest', 'fit', 'claimed', 'worker-a', ?2, ?3, ?3, ?4, ?3, ?3)",
        libsql::params![
            job_id.clone(),
            token.clone(),
            now.to_string(),
            "2026-06-29T14:02:00Z".to_string(),
        ],
    )
    .await
    .expect("insert claimed job");

    let stale = ota_jobs::finish(&conn, &job_id, "stale-token-wrong", "succeeded", None, now)
        .await
        .expect("finish");
    assert_eq!(stale, 0, "stale token must not update job");

    let job = ota_jobs::get(&conn, &job_id).await.expect("get").expect("row");
    assert_eq!(job.status, "claimed");
}

#[test]
fn capture_checksum_is_sha256_hex() {
    let c = captures::capture_checksum("hello");
    assert_eq!(c.len(), 64);
    assert!(c.chars().all(|ch| ch.is_ascii_hexdigit()));
}