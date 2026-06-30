//! Integration tests for OTA workflow repositories. Real-Turso; skips if creds absent.

use std::collections::HashMap;
use std::process::Command;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use travel_db::repo::{origin, ota_jobs, ota_source_workflow, product_type_inputs};

mod common;
use common::Guard;

static REPO_WORKFLOW_TEST_LOCK: Mutex<()> = Mutex::new(());

fn repo_workflow_test_lock() -> std::sync::MutexGuard<'static, ()> {
    REPO_WORKFLOW_TEST_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_travel"))
}

fn nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

fn is_credless(err: &str) -> bool {
    err.contains("turso auth login")
        || err.contains("Missing Turso")
        || err.contains("failed to connect to Turso")
        || err.contains("TRAVEL_TURSO")
        || err.contains("database URL")
}

fn run(args: &[&str]) -> (bool, String, String) {
    let out = bin()
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("run travel {args:?}: {e}"));
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn exec_sql(sql: &str) {
    let _ = bin().args(["db", "exec", sql]).output();
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

fn teardown(source_id: &str) {
    exec_sql(&format!(
        "DELETE FROM ota_observations WHERE job_id IN \
         (SELECT job_id FROM ota_jobs WHERE source_id='{source_id}')"
    ));
    exec_sql(&format!(
        "DELETE FROM ota_attempts WHERE job_id IN \
         (SELECT job_id FROM ota_jobs WHERE source_id='{source_id}')"
    ));
    exec_sql(&format!(
        "DELETE FROM ota_job_params WHERE job_id IN \
         (SELECT job_id FROM ota_jobs WHERE source_id='{source_id}')"
    ));
    exec_sql(&format!("DELETE FROM ota_jobs WHERE source_id='{source_id}'"));
    exec_sql(&format!(
        "DELETE FROM ota_source_workflow WHERE source_id='{source_id}'"
    ));
}

fn teardown_url_token(source_id: &str) {
    exec_sql(&format!(
        "DELETE FROM ota_source_url_token WHERE source_id='{source_id}'"
    ));
    teardown(source_id);
}

#[tokio::test]
async fn workflow_get_and_job_params_round_trip() {
    let _lock = repo_workflow_test_lock();
    let (ok, _stdout, stderr) = run(&["db", "migrate"]);
    if !ok && is_credless(&stderr) {
        eprintln!("skipping (no creds): {}", stderr.trim());
        return;
    }
    assert!(ok, "db migrate should succeed; stderr={stderr}");

    let conn = match connect_write().await {
        Ok(c) => c,
        Err(e) if is_credless(&e) => {
            eprintln!("skipping repo workflow test: {e}");
            return;
        }
        Err(e) => panic!("connect failed: {e}"),
    };

    let suffix = nanos();
    let source_id = format!("zztest{suffix}");
    let job_id = format!("zzjob{suffix}");
    let empty_job_id = format!("zzempty{suffix}");
    let now = "2026-06-30T10:00:00Z";
    teardown(&source_id);
    let _g = Guard::new({
        let source_id = source_id.clone();
        move || teardown(&source_id)
    });

    conn.execute(
        "INSERT INTO ota_source_workflow \
         (source_id, product_type, nav_kind, url_template, capture_url_contains, \
          settle_marker, settle_ms, agent_extraction_note, updated_at) \
         VALUES (?1, 'fit', 'get', ?2, ?3, ?4, 1234, ?5, ?6)",
        libsql::params![
            source_id.clone(),
            "https://example.com/search?d={depart}&r={return}".to_string(),
            "example.com/search".to_string(),
            "Loading".to_string(),
            "zz note".to_string(),
            now.to_string(),
        ],
    )
    .await
    .expect("insert workflow");

    let mut params = HashMap::new();
    params.insert("depart_date".to_string(), "2026-09-01".to_string());
    params.insert("return_date".to_string(), "2026-09-05".to_string());
    params.insert("pax".to_string(), "2".to_string());

    ota_jobs::enqueue(
        &conn,
        &ota_jobs::EnqueueInput {
            job_id: job_id.clone(),
            source_id: source_id.clone(),
            product_type: "fit".to_string(),
            params,
            now: now.to_string(),
        },
    )
    .await
    .expect("enqueue job with params");

    ota_jobs::enqueue(
        &conn,
        &ota_jobs::EnqueueInput {
            job_id: empty_job_id.clone(),
            source_id: source_id.clone(),
            product_type: "fit".to_string(),
            params: HashMap::new(),
            now: now.to_string(),
        },
    )
    .await
    .expect("enqueue empty job");

    let got = ota_source_workflow::get(&conn, &source_id, "fit")
        .await
        .expect("workflow get")
        .expect("workflow row");
    assert_eq!(got.source_id, source_id);
    assert_eq!(got.product_type, "fit");
    assert_eq!(got.nav_kind, "get");
    assert_eq!(got.url_template, "https://example.com/search?d={depart}&r={return}");
    assert_eq!(got.capture_url_contains.as_deref(), Some("example.com/search"));
    assert_eq!(got.settle_marker.as_deref(), Some("Loading"));
    assert_eq!(got.settle_ms, 1234);
    assert_eq!(got.agent_extraction_note.as_deref(), Some("zz note"));

    let missing = ota_source_workflow::get(&conn, &source_id, "group_tour")
        .await
        .expect("workflow get missing");
    assert!(missing.is_none(), "missing workflow row must return None");

    let mut got_params = ota_jobs::get_params(&conn, &job_id)
        .await
        .expect("get params");
    got_params.sort();
    assert_eq!(
        got_params,
        vec![
            ("depart_date".to_string(), "2026-09-01".to_string()),
            ("pax".to_string(), "2".to_string()),
            ("return_date".to_string(), "2026-09-05".to_string()),
        ]
    );

    let empty_params = ota_jobs::get_params(&conn, &empty_job_id)
        .await
        .expect("get empty params");
    assert!(empty_params.is_empty(), "job without params should return an empty vec");
}

#[tokio::test]
async fn claim_specific_claims_exact_job_and_region_id_looks_up_provider_code() {
    let _lock = repo_workflow_test_lock();
    let (ok, _stdout, stderr) = run(&["db", "migrate"]);
    if !ok && is_credless(&stderr) {
        eprintln!("skipping (no creds): {}", stderr.trim());
        return;
    }
    assert!(ok, "db migrate should succeed; stderr={stderr}");

    let conn = match connect_write().await {
        Ok(c) => c,
        Err(e) if is_credless(&e) => {
            eprintln!("skipping repo workflow test: {e}");
            return;
        }
        Err(e) => panic!("connect failed: {e}"),
    };

    let suffix = nanos();
    let source_id = format!("zztest{suffix}");
    let old_job_id = format!("zzold{suffix}");
    let new_job_id = format!("zznew{suffix}");
    let now = "2026-06-30T10:10:00Z";
    teardown(&source_id);
    let _g = Guard::new({
        let source_id = source_id.clone();
        move || teardown(&source_id)
    });

    conn.execute(
        "INSERT INTO ota_jobs (job_id, source_id, product_type, status, created_at, updated_at) \
         VALUES (?1, ?2, 'fit', 'queued', '2000-01-01T00:00:00Z', '2000-01-01T00:00:00Z')",
        libsql::params![old_job_id.clone(), source_id.clone()],
    )
    .await
    .expect("insert older queued job");

    ota_jobs::enqueue(
        &conn,
        &ota_jobs::EnqueueInput {
            job_id: new_job_id.clone(),
            source_id: source_id.clone(),
            product_type: "fit".to_string(),
            params: HashMap::new(),
            now: now.to_string(),
        },
    )
    .await
    .expect("enqueue newer job");

    let claimed = ota_jobs::claim_specific(
        &conn,
        &new_job_id,
        "repo-workflow-test",
        now,
        "2026-06-30T10:20:00Z",
    )
    .await
    .expect("claim specific")
    .expect("specific queued job should be claimed");

    assert_eq!(claimed.job_id, new_job_id);
    assert_eq!(claimed.source_id, source_id);
    assert_eq!(claimed.product_type, "fit");
    assert!(!claimed.claim_token.is_empty(), "claim token must be populated");

    let old_job = ota_jobs::get(&conn, &old_job_id)
        .await
        .expect("get old job")
        .expect("old job row");
    assert_eq!(old_job.status, "queued", "older queued job A must not be stolen");
    assert!(old_job.claim_token.is_none());

    let new_job = ota_jobs::get(&conn, &new_job_id)
        .await
        .expect("get new job")
        .expect("new job row");
    assert_eq!(new_job.status, "claimed");
    assert_eq!(new_job.claim_token.as_deref(), Some(claimed.claim_token.as_str()));

    let already_claimed = ota_jobs::claim_specific(
        &conn,
        &new_job_id,
        "repo-workflow-test-2",
        now,
        "2026-06-30T10:20:00Z",
    )
    .await
    .expect("claim already claimed");
    assert!(
        already_claimed.is_none(),
        "claim_specific on already-claimed job must return None"
    );

    let nonexistent = ota_jobs::claim_specific(
        &conn,
        "zzmissing-job-id",
        "repo-workflow-test-3",
        now,
        "2026-06-30T10:20:00Z",
    )
    .await
    .expect("claim nonexistent");
    assert!(
        nonexistent.is_none(),
        "claim_specific on nonexistent job must return None"
    );

    let tokyo = ota_source_workflow::region_id(&conn, "besttour", "group_tour", "東京")
        .await
        .expect("region lookup");
    assert_eq!(tokyo.as_deref(), Some("295"));

    let missing = ota_source_workflow::region_id(&conn, "besttour", "group_tour", "不存在")
        .await
        .expect("missing region lookup");
    assert!(missing.is_none(), "missing region label must return None");
}

#[tokio::test]
async fn url_token_looks_up_seeded_token_and_missing_returns_none() {
    let _lock = repo_workflow_test_lock();
    let (ok, _stdout, stderr) = run(&["db", "migrate"]);
    if !ok && is_credless(&stderr) {
        eprintln!("skipping (no creds): {}", stderr.trim());
        return;
    }
    assert!(ok, "db migrate should succeed; stderr={stderr}");

    let conn = match connect_write().await {
        Ok(c) => c,
        Err(e) if is_credless(&e) => {
            eprintln!("skipping repo workflow test: {e}");
            return;
        }
        Err(e) => panic!("connect failed: {e}"),
    };

    let suffix = nanos();
    let source_id = format!("zztest{suffix}");
    teardown_url_token(&source_id);
    let _g = Guard::new({
        let source_id = source_id.clone();
        move || teardown_url_token(&source_id)
    });

    conn.execute(
        "INSERT INTO ota_source_url_token \
         (source_id, product_type, placeholder, input_key, input_value, token_value) \
         VALUES (?1, 'fit', 'dest_code', 'destination', 'tokyo', 'TYO')",
        libsql::params![source_id.clone()],
    )
    .await
    .expect("insert url token");

    let got = ota_source_workflow::url_token(
        &conn,
        &source_id,
        "fit",
        "dest_code",
        "destination",
        "tokyo",
    )
    .await
    .expect("url token lookup");
    assert_eq!(got.as_deref(), Some("TYO"));

    let missing = ota_source_workflow::url_token(
        &conn,
        &source_id,
        "fit",
        "dest_code",
        "destination",
        "osaka",
    )
    .await
    .expect("missing url token lookup");
    assert!(missing.is_none(), "missing token row must return None");
}

#[tokio::test]
async fn product_type_inputs_list_for_type_returns_seeded_contracts_in_order() {
    let _lock = repo_workflow_test_lock();
    let (ok, _stdout, stderr) = run(&["db", "migrate"]);
    if !ok && is_credless(&stderr) {
        eprintln!("skipping (no creds): {}", stderr.trim());
        return;
    }
    assert!(ok, "db migrate should succeed; stderr={stderr}");

    let conn = match connect_write().await {
        Ok(c) => c,
        Err(e) if is_credless(&e) => {
            eprintln!("skipping repo workflow test: {e}");
            return;
        }
        Err(e) => panic!("connect failed: {e}"),
    };

    let cases: Vec<(&str, Vec<(&str, &str, i64, Option<&str>)>)> = vec![
        (
            "flight",
            vec![
                ("destination", "token_key", 1, None),
                ("depart", "common", 1, Some("caller")),
                ("return", "common", 1, Some("caller")),
                ("origin", "common", 1, Some("db")),
                ("currency", "common", 1, Some("db")),
            ],
        ),
        (
            "hotel",
            vec![
                ("destination", "token_key", 1, None),
                ("hotel", "token_key", 1, None),
                ("depart", "common", 1, Some("caller")),
                ("nights", "common", 1, Some("caller")),
                ("pax", "common", 1, Some("caller")),
                ("rooms", "common", 1, Some("code")),
                ("currency", "common", 1, Some("db")),
            ],
        ),
        (
            "fit",
            vec![
                ("destination", "token_key", 1, None),
                ("depart", "common", 1, Some("caller")),
                ("return", "common", 1, Some("caller")),
                ("pax", "common", 1, Some("caller")),
            ],
        ),
        (
            "group_tour",
            vec![("destination", "token_key", 1, None)],
        ),
    ];

    for (product_type, expected) in cases {
        let rows = product_type_inputs::list_for_type(&conn, product_type)
            .await
            .expect("list product_type_inputs");

        assert!(
            rows.iter().all(|r| r.product_type == product_type),
            "all rows must be scoped to {product_type}: {rows:?}"
        );
        assert!(
            rows.windows(2).all(|pair| {
                (pair[0].sort_order, pair[0].input_name.as_str())
                    <= (pair[1].sort_order, pair[1].input_name.as_str())
            }),
            "rows must be ordered by sort_order, input_name: {rows:?}"
        );

        let got: Vec<(&str, &str, i64, Option<&str>)> = rows
            .iter()
            .map(|r| {
                (
                    r.input_name.as_str(),
                    r.input_class.as_str(),
                    r.required,
                    r.default_source.as_deref(),
                )
            })
            .collect();
        assert_eq!(got, expected, "contract rows for {product_type}");
    }
}

#[tokio::test]
async fn origin_default_origin_airport_and_currency_reads_seeded_defaults() {
    let _lock = repo_workflow_test_lock();
    let (ok, _stdout, stderr) = run(&["db", "migrate"]);
    if !ok && is_credless(&stderr) {
        eprintln!("skipping (no creds): {}", stderr.trim());
        return;
    }
    assert!(ok, "db migrate should succeed; stderr={stderr}");

    let conn = match connect_write().await {
        Ok(c) => c,
        Err(e) if is_credless(&e) => {
            eprintln!("skipping repo workflow test: {e}");
            return;
        }
        Err(e) => panic!("connect failed: {e}"),
    };

    let got = origin::default_origin_airport_and_currency(&conn)
        .await
        .expect("default origin airport/currency");

    assert_eq!(got.slug, "taiwan");
    assert_eq!(got.airport, "TPE");
    assert_eq!(got.currency, "TWD");
}

