//! Schema test for the normalized OTA provider catalog tables (DB-centric provider
//! architecture, spec 2026-06-29). After `db migrate`, the 5 new tables must exist with
//! the documented columns, and ota_source_coverage must reject proven outside {0,1}.
//!
//! Real-Turso integration test; skips cleanly if creds absent. Mirrors db_seed.rs.

use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_travel"))
}

fn is_credless(stderr: &str) -> bool {
    stderr.contains("turso auth login")
        || stderr.contains("Missing Turso")
        || stderr.contains("failed to connect to Turso")
        || stderr.contains("TRAVEL_TURSO")
}

/// Run a `travel` subcommand → (ok, stdout, stderr).
fn run(args: &[&str]) -> (bool, String, String) {
    let out = bin().args(args).output().unwrap_or_else(|e| panic!("run travel {args:?}: {e}"));
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// `db schema <table>` output → the set of column names it lists.
fn columns_of(table: &str) -> Option<Vec<String>> {
    let (ok, stdout, stderr) = run(&["db", "schema", table]);
    if !ok {
        if is_credless(&stderr) {
            eprintln!("skipping ota-catalog-schema test: {}", stderr.trim());
            return None;
        }
        panic!("db schema {table} failed: {}", stderr.trim());
    }
    // Lines look like "  code  TEXT  [PK]"; the first token on an indented line is the col.
    let cols: Vec<String> = stdout
        .lines()
        .filter(|l| l.starts_with("  ") && !l.contains("columns)"))
        .filter_map(|l| l.split_whitespace().next().map(|s| s.to_string()))
        .collect();
    Some(cols)
}

#[tokio::test]
async fn catalog_tables_exist_with_documented_columns() {
    // Ensure migrate has run (idempotent).
    let (ok, _o, err) = run(&["db", "migrate"]);
    if !ok && is_credless(&err) {
        eprintln!("skipping ota-catalog-schema test (no creds): {}", err.trim());
        return;
    }

    let expected: &[(&str, &[&str])] = &[
        ("product_types", &["code", "description"]),
        ("coverage_block_reasons", &["code", "description"]),
        (
            "ota_source_coverage",
            &[
                "source_id",
                "product_type",
                "proven",
                "proven_at",
                "method",
                "search_url",
                "blocked_reason_code",
                "updated_at",
            ],
        ),
        (
            "ota_source_region_codes",
            &["source_id", "product_type", "region_label", "region_code"],
        ),
        (
            "catalog_runs",
            &["run_id", "command_type", "command_summary", "status", "changed_at"],
        ),
    ];

    for (table, want_cols) in expected {
        let Some(have) = columns_of(table) else {
            return; // credless mid-test
        };
        for col in *want_cols {
            assert!(
                have.iter().any(|c| c == col),
                "table {table} must have column {col}; got {have:?}"
            );
        }
    }
}

#[tokio::test]
async fn coverage_rejects_invalid_proven() {
    let (ok, _o, err) = run(&["db", "migrate"]);
    if !ok && is_credless(&err) {
        eprintln!("skipping (no creds): {}", err.trim());
        return;
    }
    // proven=2 violates CHECK(proven IN (0,1)) → the insert must FAIL.
    let (ok, stdout, stderr) = run(&[
        "db",
        "exec",
        "INSERT INTO ota_source_coverage (source_id, product_type, proven) VALUES ('zzcheck','fit',2)",
    ]);
    if is_credless(&stderr) {
        eprintln!("skipping (no creds mid-test): {}", stderr.trim());
        return;
    }
    assert!(
        !ok || stderr.to_lowercase().contains("constraint") || stdout.to_lowercase().contains("error"),
        "proven=2 must be rejected by the CHECK; ok={ok} stdout={stdout} stderr={stderr}"
    );
    // Cleanup any stray row (in case the CHECK was somehow not enforced).
    let _ = run(&["db", "exec", "DELETE FROM ota_source_coverage WHERE source_id='zzcheck'"]);
}
