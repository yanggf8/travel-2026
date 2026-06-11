// `travel db cleanup-deleted [--confirm]` — batched HARD-wipe of soft-deleted
// plans (those with plans.deleted_at IS NOT NULL).
//
// Dry-run ergonomics (SAFEST choice): this defaults to DRY-RUN. A bare
// `db cleanup-deleted` only COUNTS and reports what WOULD be deleted, changing
// nothing. An explicit `--confirm` (alias `--yes`) is required to actually
// delete. `--dry-run` is also accepted as an explicit no-op flag for symmetry.
//
// Plan-scoped tables are DISCOVERED at runtime: every table carrying a `plan_id`
// column (via PRAGMA table_info) is wiped for each flagged plan. `plans` itself
// is deleted LAST (it holds the deleted_at flag we keyed on). This avoids a
// hardcoded ~55-table list drifting out of sync with the schema.
//
// Honesty: per-plan, per-table counts are ALWAYS shown including 0, plus a grand
// total. If no plans are flagged, that is stated explicitly (never a silent
// no-op).

use libsql::Connection;

pub async fn run(args: &[String]) -> Result<(), String> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print_usage();
        return Ok(());
    }

    let confirm = args.iter().any(|a| a == "--confirm" || a == "--yes");
    let explicit_dry = args.iter().any(|a| a == "--dry-run");
    for a in args {
        if !matches!(a.as_str(), "--confirm" | "--yes" | "--dry-run") {
            return Err(format!(
                "unknown argument: {a}\nUsage: travel db cleanup-deleted [--confirm]"
            ));
        }
    }
    // Dry-run unless --confirm/--yes. (--dry-run is redundant-but-allowed.)
    let dry_run = !confirm || explicit_dry;

    let conn = crate::db::connect_write()
        .await
        .map_err(|e| format!("failed to connect to Turso (write tier): {e}"))?;

    // 1. Which plans are flagged?
    let flagged = flagged_plans(&conn).await?;
    if flagged.is_empty() {
        println!("No plans are flagged for deletion (plans.deleted_at IS NULL for all).");
        println!("Nothing to clean up.");
        return Ok(());
    }

    // 2. Which tables are plan-scoped? (discovered at runtime)
    let mut tables = plan_scoped_tables(&conn).await?;
    // Delete child rows first; `plans` (the flag holder) last.
    tables.retain(|t| t != "plans");
    tables.sort();
    tables.push("plans".to_string());

    if dry_run {
        println!("DRY RUN — no rows will be deleted. Re-run with --confirm to wipe.");
    } else {
        println!("WIPING {} soft-deleted plan(s) — this is irreversible.", flagged.len());
    }
    println!();

    let mut grand_total: i64 = 0;
    for plan_id in &flagged {
        println!("plan: {plan_id}");
        let mut plan_total: i64 = 0;
        for table in &tables {
            let n = if dry_run {
                count_rows(&conn, table, plan_id).await?
            } else {
                delete_rows(&conn, table, plan_id).await?
            };
            plan_total += n;
            let verb = if dry_run { "would delete" } else { "deleted" };
            println!("  {table}: {verb} {n} row(s)");
        }
        let verb = if dry_run { "would delete" } else { "deleted" };
        println!("  → plan total: {verb} {plan_total} row(s)");
        println!();
        grand_total += plan_total;
    }

    let verb = if dry_run { "would delete" } else { "deleted" };
    println!(
        "GRAND TOTAL: {verb} {grand_total} row(s) across {} plan(s) and {} table(s).",
        flagged.len(),
        tables.len()
    );
    if dry_run {
        println!("(dry run — nothing was changed. Re-run with --confirm to perform the wipe.)");
    }
    Ok(())
}

fn print_usage() {
    println!(
        "Usage:\n  travel db cleanup-deleted [--confirm]\n\n\
         Hard-wipes all soft-deleted plans (plans.deleted_at IS NOT NULL) across every\n\
         plan-scoped table. Defaults to a DRY RUN; pass --confirm (or --yes) to delete.\n\
         Plan-scoped tables are discovered at runtime (any table with a plan_id column)."
    );
}

async fn flagged_plans(conn: &Connection) -> Result<Vec<String>, String> {
    let mut rows = conn
        .query(
            "SELECT plan_id FROM plans WHERE deleted_at IS NOT NULL ORDER BY plan_id",
            (),
        )
        .await
        .map_err(|e| format!("flagged-plans query failed: {e}"))?;
    let mut out = Vec::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("flagged-plans row read failed: {e}"))?
    {
        let id: String = row.get(0).unwrap_or_default();
        if !id.is_empty() {
            out.push(id);
        }
    }
    Ok(out)
}

/// Every table carrying a `plan_id` column. Discovered via sqlite_master +
/// PRAGMA table_info so the wipe set tracks the live schema.
async fn plan_scoped_tables(conn: &Connection) -> Result<Vec<String>, String> {
    let mut names = Vec::new();
    let mut rows = conn
        .query(
            "SELECT name FROM sqlite_master WHERE type='table' \
             AND name NOT LIKE 'sqlite_%' ORDER BY name",
            (),
        )
        .await
        .map_err(|e| format!("table list query failed: {e}"))?;
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("table list row read failed: {e}"))?
    {
        let name: String = row.get(0).unwrap_or_default();
        if !name.is_empty() {
            names.push(name);
        }
    }

    let mut scoped = Vec::new();
    for name in names {
        if table_has_plan_id(conn, &name).await? {
            scoped.push(name);
        }
    }
    Ok(scoped)
}

async fn table_has_plan_id(conn: &Connection, table: &str) -> Result<bool, String> {
    // table names come from sqlite_master (trusted); inline is acceptable for PRAGMA.
    let sql = format!("PRAGMA table_info({table});");
    let mut rows = conn
        .query(&sql, ())
        .await
        .map_err(|e| format!("PRAGMA table_info({table}) failed: {e}"))?;
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("PRAGMA row read failed: {e}"))?
    {
        // table_info cols: cid, name, type, notnull, dflt_value, pk.
        if let Ok(libsql::Value::Text(col)) = row.get_value(1) {
            if col == "plan_id" {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

async fn count_rows(conn: &Connection, table: &str, plan_id: &str) -> Result<i64, String> {
    let sql = format!("SELECT COUNT(*) AS n FROM {table} WHERE plan_id = ?1");
    let mut rows = conn
        .query(&sql, libsql::params![plan_id.to_string()])
        .await
        .map_err(|e| format!("COUNT on {table} failed: {e}"))?;
    if let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("COUNT row read on {table} failed: {e}"))?
    {
        let n: i64 = row.get(0).unwrap_or(0);
        return Ok(n);
    }
    Ok(0)
}

async fn delete_rows(conn: &Connection, table: &str, plan_id: &str) -> Result<i64, String> {
    let sql = format!("DELETE FROM {table} WHERE plan_id = ?1");
    let affected = conn
        .execute(&sql, libsql::params![plan_id.to_string()])
        .await
        .map_err(|e| format!("DELETE on {table} failed: {e}"))?;
    Ok(affected as i64)
}
