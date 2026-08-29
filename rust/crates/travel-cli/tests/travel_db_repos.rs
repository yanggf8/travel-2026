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
async fn offer_reingest_dedupes_and_bumps_last_seen() {
    let conn = match connect_write().await {
        Ok(c) => c,
        Err(e) if is_credless(&e) => {
            eprintln!("skipping offer reingest test: {e}");
            return;
        }
        Err(e) => panic!("connect failed: {e}"),
    };

    let offer_id = format!("zzrepo{}", nanos());
    let t1 = "2026-09-01T08:00:00Z";
    let t2 = "2026-09-05T08:00:00Z";
    teardown_offer(&conn, &offer_id, t1).await;
    teardown_offer(&conn, &offer_id, t2).await;
    let _g = Guard::new({
        let offer_id = offer_id.clone();
        move || {
            exec_sql(&format!("DELETE FROM offers WHERE id='{offer_id}'"));
        }
    });

    let mk = |scraped_at: &str, key: String| offers::OfferRow {
        id: offer_id.clone(),
        source_id: "zztest".to_string(),
        offer_type: "package".to_string(),
        scraped_at: scraped_at.to_string(),
        offer_key: Some(key),
        hotel_name: Some("Hotel Dedup".to_string()),
        price_per_person: Some(25500),
        currency: Some("TWD".to_string()),
        destination: Some("zz_dest".to_string()),
        departure_date: Some("2026-09-01".to_string()),
        nights: Some(3),
        ..Default::default()
    };

    let first = offers::insert(&conn, &mk(t1, offer_id.clone())).await.expect("first insert");
    assert_eq!(first.inserted, 1);
    // The re-ingest shape: identical business content under a fresh scraped_at.
    let again = offers::insert(&conn, &mk(t2, offer_id.clone())).await.expect("re-ingest");
    assert_eq!(again.inserted, 0);
    assert_eq!(again.deduped, 1);

    let mut rows = conn
        .query(
            "SELECT scraped_at, last_seen_at FROM offers WHERE id = ?1",
            libsql::params![offer_id.clone()],
        )
        .await
        .expect("query");
    let mut seen = Vec::new();
    while let Some(row) = rows.next().await.expect("row") {
        let scraped: String = row.get(0).unwrap_or_default();
        let seen_at: String = row.get(1).unwrap_or_default();
        seen.push((scraped, seen_at));
    }
    assert_eq!(seen.len(), 1, "re-ingest must not add a row");
    assert_eq!(seen[0].0, t1, "first-seen scraped_at holds");
    assert_eq!(seen[0].1, t2, "last_seen_at bumped to the new observation");
}

#[tokio::test]
async fn offer_price_change_is_a_new_row_not_a_dedup() {
    let conn = match connect_write().await {
        Ok(c) => c,
        Err(e) if is_credless(&e) => {
            eprintln!("skipping price-change test: {e}");
            return;
        }
        Err(e) => panic!("connect failed: {e}"),
    };

    let offer_id = format!("zzrepo{}", nanos());
    let t1 = "2026-09-01T08:00:00Z";
    let t2 = "2026-09-05T08:00:00Z";
    teardown_offer(&conn, &offer_id, t1).await;
    teardown_offer(&conn, &offer_id, t2).await;
    let _g = Guard::new({
        let offer_id = offer_id.clone();
        move || {
            exec_sql(&format!("DELETE FROM offers WHERE id='{offer_id}'"));
        }
    });

    let mk = |scraped_at: &str, price: i64| offers::OfferRow {
        id: offer_id.clone(),
        source_id: "zztest".to_string(),
        offer_type: "package".to_string(),
        scraped_at: scraped_at.to_string(),
        offer_key: Some(offer_id.clone()),
        price_per_person: Some(price),
        currency: Some("TWD".to_string()),
        destination: Some("zz_dest".to_string()),
        departure_date: Some("2026-09-01".to_string()),
        nights: Some(3),
        ..Default::default()
    };

    let first = offers::insert(&conn, &mk(t1, 25500)).await.expect("first insert");
    assert_eq!(first.inserted, 1);
    // Price is INSIDE the hash: a price change must be a new price point, never silently
    // dropped by the dedup UNIQUE index.
    let changed = offers::insert(&conn, &mk(t2, 26900)).await.expect("price change");
    assert_eq!(changed.inserted, 1);
    assert_eq!(changed.deduped, 0);

    let mut rows = conn
        .query(
            "SELECT price_per_person, dedup_key FROM offers WHERE id = ?1 ORDER BY scraped_at",
            libsql::params![offer_id.clone()],
        )
        .await
        .expect("query");
    let mut keys = Vec::new();
    while let Some(row) = rows.next().await.expect("row") {
        let price: i64 = row.get(0).unwrap_or_default();
        let key: String = row.get(1).unwrap_or_default();
        keys.push((price, key));
    }
    assert_eq!(keys.len(), 2, "price history is two rows");
    assert_ne!(keys[0].1, keys[1].1, "distinct content keys");
    assert_eq!(keys[0].0, 25500);
    assert_eq!(keys[1].0, 26900);
}

#[tokio::test]
async fn offers_columns_are_all_hash_or_excluded() {
    // Drift guard against the LIVE schema: every column must be a decided member of
    // DEDUP_HASH_FIELDS or DEDUP_EXCLUDED_FIELDS, so a future column can't silently escape
    // the content hash.
    let conn = match connect_write().await {
        Ok(c) => c,
        Err(e) if is_credless(&e) => {
            eprintln!("skipping column-drift test: {e}");
            return;
        }
        Err(e) => panic!("connect failed: {e}"),
    };
    let mut rows = conn
        .query("PRAGMA table_info(offers)", ())
        .await
        .expect("pragma");
    let mut cols = Vec::new();
    while let Some(row) = rows.next().await.expect("row") {
        let name: String = row.get(1).unwrap_or_default();
        cols.push(name);
    }
    assert!(
        cols.contains(&"dedup_key".to_string()),
        "run `db migrate` first: dedup columns missing (found {cols:?})"
    );
    let decided: std::collections::HashSet<&str> = offers::DEDUP_HASH_FIELDS
        .iter()
        .chain(offers::DEDUP_EXCLUDED_FIELDS)
        .copied()
        .collect();
    for c in &cols {
        assert!(
            decided.contains(c.as_str()),
            "offers column {c} is not in DEDUP_HASH_FIELDS or DEDUP_EXCLUDED_FIELDS — decide whether it is content"
        );
    }
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