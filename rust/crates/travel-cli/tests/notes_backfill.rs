//! Regression test for the two-phase ota_sources.notes backfill into normalized coverage.
//! This asserts global catalog rows in Turso; it does not create plan-scoped data.

use std::process::Command;

mod common;
use common::Guard;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_travel"))
}

fn is_credless(stderr: &str) -> bool {
    stderr.contains("turso auth login")
        || stderr.contains("Missing Turso")
        || stderr.contains("failed to connect to Turso")
        || stderr.contains("TRAVEL_TURSO")
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

fn scalar(sql: &str) -> Option<String> {
    let (ok, stdout, stderr) = run(&["db", "exec", sql]);
    if !ok {
        if is_credless(&stderr) {
            return None;
        }
        panic!("db exec failed: {}\nSQL: {sql}", stderr.trim());
    }
    stdout
        .lines()
        .find_map(|l| l.split_once(':').map(|(_, v)| v.trim().to_string()))
}

#[tokio::test]
async fn notes_backfill_populates_coverage_and_audit_rows() {
    let _g = Guard::new(|| {});
    let (ok, _out, err) = run(&["db", "migrate"]);
    if !ok && is_credless(&err) {
        eprintln!("skipping (no creds): {}", err.trim());
        return;
    }
    assert!(ok, "db migrate should succeed; err={err}");

    for (source_id, product_type) in [
        ("google_flights", "flight"),
        ("agoda", "hotel"),
        ("eztravel", "fit"),
        ("settour", "fit"),
        ("besttour", "group_tour"),
        ("travel4u", "group_tour"),
    ] {
        let sql = format!(
            "SELECT count(*) AS n FROM ota_source_coverage \
             WHERE source_id='{source_id}' AND product_type='{product_type}' \
               AND proven=1 AND proven_at IS NOT NULL AND method='agent_parse'"
        );
        assert_eq!(
            scalar(&sql).as_deref(),
            Some("1"),
            "{source_id}/{product_type} must have proven agent_parse coverage"
        );
    }

    for (source_id, product_type, blocked) in [
        ("liontravel", "fit", "renderer_wedge"),
        ("lifetour", "group_tour", "renderer_wedge"),
        ("tigerair", "flight", "redundant"),
        ("trip", "flight", "redundant"),
        ("booking", "hotel", "cloudflare"),
        ("skyscanner", "flight", "captcha"),
        ("jalan", "hotel", "unsupported"),
        ("rakuten_travel", "hotel", "unsupported"),
    ] {
        let sql = format!(
            "SELECT count(*) AS n FROM ota_source_coverage \
             WHERE source_id='{source_id}' AND product_type='{product_type}' \
               AND blocked_reason_code='{blocked}'"
        );
        assert_eq!(
            scalar(&sql).as_deref(),
            Some("1"),
            "{source_id}/{product_type} must have blocked_reason_code={blocked}"
        );
    }

    assert_eq!(
        scalar("SELECT count(DISTINCT source_id) AS n FROM ota_notes_migration_audit").as_deref(),
        Some("14"),
        "each ota_sources.notes row must be recorded in ota_notes_migration_audit"
    );
}
