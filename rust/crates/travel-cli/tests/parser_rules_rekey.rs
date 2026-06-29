//! Integration test for re-keying parser_rules from source_id to (source_id, product_type).
//! Real-Turso; skips if creds absent. Test-owned rows use zz* and Guard teardown.

use std::process::Command;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

mod common;
use common::Guard;

static PARSER_RULES_LOCK: Mutex<()> = Mutex::new(());

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_travel"))
}

fn nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos()
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

fn teardown(source_id: &str) {
    let _ = run(&[
        "db",
        "exec",
        &format!("DELETE FROM parser_rules WHERE source_id='{source_id}'"),
    ]);
    let _ = run(&[
        "db",
        "exec",
        &format!("DELETE FROM ota_sources WHERE source_id='{source_id}'"),
    ]);
}

#[tokio::test]
async fn parser_rules_are_keyed_per_product_type() {
    let _lock = PARSER_RULES_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let (ok, _out, err) = run(&["db", "migrate"]);
    if !ok && is_credless(&err) {
        eprintln!("skipping (no creds): {}", err.trim());
        return;
    }
    assert!(ok, "db migrate should succeed; err={err}");

    let (ok, schema, err) = run(&["db", "schema", "parser_rules"]);
    assert!(ok, "db schema parser_rules should succeed; err={err}");
    let source_pk = schema
        .lines()
        .any(|line| line.contains("source_id") && line.contains("PK"));
    let product_type_pk = schema
        .lines()
        .any(|line| line.contains("product_type") && line.contains("PK"));
    assert!(source_pk, "source_id must be part of the PK:\n{schema}");
    assert!(
        product_type_pk,
        "product_type must be part of the PK:\n{schema}"
    );
    assert!(
        !schema.contains("product_kind"),
        "old product_kind column must be gone:\n{schema}"
    );

    let source_id = format!("zzparser{}", nanos());
    teardown(&source_id);
    let _g = Guard::new({
        let source_id = source_id.clone();
        move || teardown(&source_id)
    });

    let insert_source = format!(
        "INSERT INTO ota_sources (source_id, name, status) VALUES ('{source_id}', 'ZZ Parser', 'active')"
    );
    let (ok, _out, err) = run(&["db", "exec", &insert_source]);
    assert!(ok, "seed ota_sources row should succeed; err={err}");

    for product_type in ["fit", "flight"] {
        let insert_rule = format!(
            "INSERT INTO parser_rules \
             (source_id, product_type, date_range_rx, nights_rx, price_marker, price_amount_rx, flight_rx, hotel_anchor_rx) \
             VALUES ('{source_id}', '{product_type}', '.', '.', 'TWD', '([0-9]+)', '.', '.')"
        );
        let (ok, _out, err) = run(&["db", "exec", &insert_rule]);
        assert!(
            ok,
            "insert parser_rules {product_type} row should succeed; err={err}"
        );
    }

    assert_eq!(
        scalar(&format!(
            "SELECT count(*) AS n FROM parser_rules WHERE source_id='{source_id}'"
        ))
        .as_deref(),
        Some("2"),
        "one source should hold two product-specific parser rules"
    );
    assert_eq!(
        scalar("SELECT product_type FROM parser_rules WHERE source_id='besttour'").as_deref(),
        Some("group_tour"),
        "besttour parser rule must be reconciled to group_tour"
    );
}
