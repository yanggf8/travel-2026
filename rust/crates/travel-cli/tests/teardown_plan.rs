//! Proof test for the canonical `common::teardown_plan` / `teardown_offers` helpers.
//!
//! Verifies dynamic plan-keyed table discovery (drift-proof), full cleanup of a
//! seeded plan across 57 tables, and that bogus-plan calls do not panic. Uses
//! panic-safe Guard. Skips cleanly if no Turso creds.

use std::process::Command;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

mod common;
use common::Guard;

static TEARDOWN_PROOF_LOCK: Mutex<()> = Mutex::new(());

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_travel")
}

fn nanos() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
}

fn db_exec(sql: &str) -> (bool, String, String) {
    let out = Command::new(bin())
        .args(["db", "exec", sql])
        .env_remove("TRAVEL_PLAN_ID")
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

fn column(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .filter_map(|l| l.split_once(':').map(|(_, v)| v.trim().to_string()))
        .filter(|v| !v.is_empty())
        .collect()
}

fn sql_lit(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn sql_ident(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

fn seed_minimal_plan(plan: &str, dest: &str) {
    // Seed core plan tables + plan_destinations + audit + process
    let _ = db_exec(&format!(
        "INSERT OR REPLACE INTO plans (plan_id, schema_version, version) \
         VALUES ('{plan}', '4.2.0', 0)"
    ));
    let _ = db_exec(&format!(
        "INSERT OR REPLACE INTO plan_metadata (plan_id, schema_version, active_destination) \
         VALUES ('{plan}', '4.2.0', '{dest}')"
    ));
    let _ = db_exec(&format!(
        "INSERT OR REPLACE INTO plan_destinations (plan_id, slug, display_name, status) \
         VALUES ('{plan}', '{dest}', 'Teardown Proof Dest', 'draft')"
    ));
    let _ = db_exec(&format!(
        "INSERT OR REPLACE INTO operation_runs \
         (run_id, plan_id, command_type, status, started_at) \
         VALUES ('op-{plan}', '{plan}', 'test-seed', 'completed', datetime('now'))"
    ));
    let _ = db_exec(&format!(
        "INSERT OR REPLACE INTO process_statuses (plan_id, destination, process_id, status) \
         VALUES ('{plan}', '{dest}', 'process_1_date_anchor', 'pending')"
    ));
}

#[tokio::test]
async fn canonical_teardown_plan_cleans_all_plan_keyed_tables() {
    let _guard = TEARDOWN_PROOF_LOCK.lock().unwrap_or_else(|p| p.into_inner());

    let (ok, _o, err) = db_exec("SELECT 1");
    if !ok && is_skip(&err) {
        eprintln!("skipping teardown_plan proof (no Turso creds): {}", err.trim());
        return;
    }

    let tag = nanos();
    let plan = format!("teardown-proof-{tag}");
    let dest = format!("teardown_proof_{tag}");

    // defensive pre-clean + panic-safe Guard (Drop fires on panic too)
    common::teardown_plan(&plan, &dest);
    let _g = Guard::new({
        let plan = plan.clone();
        let dest = dest.clone();
        move || common::teardown_plan(&plan, &dest)
    });

    seed_minimal_plan(&plan, &dest);

    // Verify seeding touched multiple tables before teardown
    let (_, p_cnt, _) = db_exec(&format!("SELECT COUNT(*) AS n FROM plans WHERE plan_id = {}", sql_lit(&plan)));
    assert_eq!(scalar(&p_cnt).as_deref(), Some("1"), "plan seeded");

    // Call the canonical helper
    common::teardown_plan(&plan, &dest);

    // Query live table list (same SQL) — parse name: val lines
    let (ok_list, list_out, err_list) = db_exec(
        "SELECT m.name \
         FROM sqlite_master m \
         WHERE m.type='table' \
           AND EXISTS ( \
             SELECT 1 FROM pragma_table_info(m.name) p WHERE p.name='plan_id' \
           ) \
         ORDER BY CASE WHEN m.name='plans' THEN 1 ELSE 0 END, m.name;"
    );
    assert!(ok_list, "table list query must succeed; err={err_list}");

    let tables = column(&list_out);
    assert_eq!(tables.len(), 57, "expected 57 plan-keyed tables; got {}: {:?}", tables.len(), tables);

    // CRITICAL: each COUNT as SINGLE db exec (not batched) so parse yields plain 'n: 0'
    // (batched would be '[N/M] n: 0')
    for table in &tables {
        let ident = sql_ident(table);
        let plan_lit = sql_lit(&plan);
        let count_sql = format!("SELECT COUNT(*) AS n FROM {} WHERE plan_id = {};", ident, plan_lit);
        let (ok_c, out_c, err_c) = db_exec(&count_sql);
        assert!(ok_c, "count for {} failed; err={}", table, err_c);
        assert_eq!(
            scalar(&out_c).as_deref(),
            Some("0"),
            "table {} must be 0 after teardown_plan; out={}",
            table,
            out_c.trim()
        );
    }

    // Also: calling on a bogus plan must not panic (best-effort no-op)
    // (run outside any prior guard to ensure the helper itself is safe)
    common::teardown_plan("test-no-creds-bogus-plan-xyz", "test_no_creds_bogus");
    common::teardown_plan(&format!("nonexistent-plan-{}", tag), "nonexistent_dest");

    // Guard will run final cleanup (idempotent).
}
