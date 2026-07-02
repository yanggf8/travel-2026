// `travel mark-plan-deleted <plan_id> [--force]` — SOFT-delete a plan.
//
// Sets plans.deleted_at = datetime('now') so the plan stops resolving / listing
// (see plan_resolver.rs + plans.rs `deleted_at IS NULL` filter), WITHOUT wiping
// any data. The batched hard-wipe is a separate, deliberate pass
// (`travel db cleanup-deleted`).
//
// Safety rails:
//   - FAIL LOUD if the plan_id is not in `plans` (never flag a phantom).
//   - REFUSE (without --force) if the plan looks like a real upcoming/active
//     trip — i.e. it has a date_anchor whose end_date >= today. Junk test plans
//     typically have no date anchor, so they flag freely.
//   - Idempotent: marking an already-deleted plan reports that and exits 0.
//
// Audit decision: a soft-delete is a plan-lifecycle STATUS change, not a
// domain/itinerary mutation, so it does NOT emit plan_events (those are keyed to
// processes/destinations and a lifecycle flag has no process to attach to). It
// DOES write the durable audit trail every mutation owes: bump plans.version and
// INSERT an operation_runs row (run_id / command_type / status /
// version_before-after). This mirrors the lightest audit a mutation can carry
// while still recording who/what/when.

use crate::cascade::common::now_db_datetime;
use libsql::Connection;

pub async fn run(args: &[String], _resolved_plan_id: String) -> Result<(), String> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print_usage();
        return Ok(());
    }

    let parsed = parse_args(args)?;

    let conn = crate::db::connect_write()
        .await
        .map_err(|e| format!("failed to connect to Turso (write tier): {e}"))?;

    // 1. FAIL LOUD if the plan does not exist.
    let Some((version_before, already_deleted)) =
        read_plan(&conn, &parsed.plan_id).await?
    else {
        eprintln!(
            "Error: plan '{}' not found in plans — nothing to mark deleted.",
            parsed.plan_id
        );
        eprintln!("       (run `travel plans` to list existing plans)");
        std::process::exit(1);
    };

    // 2. Idempotent: already soft-deleted → report and exit 0.
    if already_deleted {
        println!(
            "{} is already marked deleted (soft) — no change. \
             run 'db cleanup-deleted' to wipe.",
            parsed.plan_id
        );
        return Ok(());
    }

    // 3. SAFETY: refuse a real upcoming/active trip without --force.
    if !parsed.force {
        if let Some(end_date) = upcoming_anchor_end(&conn, &parsed.plan_id).await? {
            eprintln!(
                "Error: '{}' has a date anchor ending {end_date} (today or later) — \
                 it looks like a real upcoming/active trip.",
                parsed.plan_id
            );
            eprintln!(
                "       Refusing to soft-delete without --force. \
                 Re-run with --force to override:"
            );
            eprintln!("         travel mark-plan-deleted {} --force", parsed.plan_id);
            std::process::exit(1);
        }
    }

    // 4. Apply: set deleted_at, write audit row, bump version.
    let version_after = version_before + 1;
    let now_db = now_db_datetime();

    travel_db::repo::plan_lifecycle::soft_delete(&conn, &parsed.plan_id, &now_db).await?;

    let summary = if parsed.force {
        "soft-delete (forced)"
    } else {
        "soft-delete"
    };
    crate::cascade::common::record_operation(
        &conn,
        &parsed.plan_id,
        "mark-plan-deleted",
        summary,
        version_before,
        version_after,
        &now_db,
    )
    .await?;

    println!(
        "marked {} deleted (soft) — data retained; run 'db cleanup-deleted' to wipe",
        parsed.plan_id
    );
    Ok(())
}

#[derive(Debug)]
struct ParsedArgs {
    plan_id: String,
    force: bool,
}

fn parse_args(args: &[String]) -> Result<ParsedArgs, String> {
    let mut plan_id: Option<String> = None;
    let mut force = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--force" => {
                force = true;
                i += 1;
            }
            // `--plan-id <id>` is honored too (the resolver consumes it
            // upstream, but accept it here so the positional and flag forms
            // both work).
            "--plan-id" => {
                plan_id = Some(
                    args.get(i + 1)
                        .ok_or_else(|| "missing value for --plan-id".to_string())?
                        .clone(),
                );
                i += 2;
            }
            other if other.starts_with("--") => {
                return Err(format!("unknown argument: {other}"));
            }
            other => {
                if plan_id.is_none() {
                    plan_id = Some(other.to_string());
                }
                i += 1;
            }
        }
    }
    let plan_id = plan_id.ok_or_else(|| {
        "Error: mark-plan-deleted requires a <plan_id>\n\
         Example: travel mark-plan-deleted test-junk-123 [--force]"
            .to_string()
    })?;
    Ok(ParsedArgs { plan_id, force })
}

fn print_usage() {
    println!(
        "Usage:\n  travel mark-plan-deleted <plan_id> [--force]\n\n\
         Soft-deletes a plan (sets plans.deleted_at). The plan stops listing/resolving\n\
         but its data is retained until `travel db cleanup-deleted` wipes it.\n\
         --force overrides the safety refusal for a real upcoming/active trip."
    );
}

/// Returns Some((version, already_deleted)) if the plan exists, None otherwise.
async fn read_plan(conn: &Connection, plan_id: &str) -> Result<Option<(i64, bool)>, String> {
    let mut rows = conn
        .query(
            "SELECT version, deleted_at FROM plans WHERE plan_id = ?1",
            libsql::params![plan_id.to_string()],
        )
        .await
        .map_err(|e| format!("plans query failed: {e}"))?;
    if let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("plans row read failed: {e}"))?
    {
        let version: i64 = row.get(0).unwrap_or(0);
        let deleted_at: Option<String> = row.get(1).ok();
        let already_deleted = deleted_at.map(|s| !s.is_empty()).unwrap_or(false);
        return Ok(Some((version, already_deleted)));
    }
    Ok(None)
}

/// If the plan has any date_anchor with end_date >= today, return that end_date
/// (the safety guard). Otherwise None.
async fn upcoming_anchor_end(
    conn: &Connection,
    plan_id: &str,
) -> Result<Option<String>, String> {
    let today = crate::plan_resolver::today_iso();
    let mut rows = conn
        .query(
            "SELECT MAX(end_date) AS m FROM date_anchors \
             WHERE plan_id = ?1 AND end_date >= ?2",
            libsql::params![plan_id.to_string(), today],
        )
        .await
        .map_err(|e| format!("date_anchors query failed: {e}"))?;
    if let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("date_anchors row read failed: {e}"))?
    {
        let end: Option<String> = row.get(0).ok();
        return Ok(end.filter(|s| !s.is_empty()));
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_positional_plan_id() {
        let p = parse_args(&["test-junk-1".to_string()]).unwrap();
        assert_eq!(p.plan_id, "test-junk-1");
        assert!(!p.force);
    }

    #[test]
    fn parse_force_flag() {
        let p = parse_args(&["test-junk-1".to_string(), "--force".to_string()]).unwrap();
        assert_eq!(p.plan_id, "test-junk-1");
        assert!(p.force);
        // order-independent
        let p2 = parse_args(&["--force".to_string(), "test-junk-1".to_string()]).unwrap();
        assert_eq!(p2.plan_id, "test-junk-1");
        assert!(p2.force);
    }

    #[test]
    fn parse_plan_id_flag() {
        let p = parse_args(&["--plan-id".to_string(), "abc".to_string()]).unwrap();
        assert_eq!(p.plan_id, "abc");
    }

    #[test]
    fn parse_missing_plan_id_errors() {
        assert!(parse_args(&[]).is_err());
        assert!(parse_args(&["--force".to_string()]).is_err());
    }

    #[test]
    fn parse_unknown_flag_errors() {
        assert!(parse_args(&["x".to_string(), "--bogus".to_string()]).is_err());
    }
}
