//! `db seed plans` — port of scripts/seed-plans-current.ts.
//!
//! Seeds the `plans` table with plan_id + schema_version only (no blobs).
//! All data lives in normalized tables. This script ensures the `plans`
//! row exists for each known plan_id. Idempotent: upserts via
//! `ON CONFLICT(plan_id) DO UPDATE`.
//!
//! Signature fixed by main.rs dispatch: `run(&[String])`.

use crate::db::connect_write;

/// Known plans — add new plan IDs here (mirrors the TS `plans` array).
const PLANS: &[(&str, &str)] = &[("tokyo-2026", "4.2.0"), ("kyoto-2026", "4.2.0")];

pub async fn run(args: &[String]) -> Result<(), String> {
    let _ = args;

    let conn = connect_write().await?;

    println!("Seeding {} plan(s) into plans table...", PLANS.len());

    for (plan_id, schema_version) in PLANS {
        println!("  - {plan_id} (schema {schema_version})");
        conn.execute(
            "INSERT INTO plans (plan_id, schema_version, updated_at)
             VALUES (?1, ?2, datetime('now'))
             ON CONFLICT(plan_id) DO UPDATE SET
               schema_version = ?2,
               updated_at = datetime('now')",
            libsql::params![plan_id.to_string(), schema_version.to_string()],
        )
        .await
        .map_err(|e| format!("plans upsert failed for {plan_id}: {e}"))?;
    }

    let ids: Vec<&str> = PLANS.iter().map(|(id, _)| *id).collect();
    println!("Done. Plan rows exist for: {}", ids.join(", "));
    Ok(())
}
