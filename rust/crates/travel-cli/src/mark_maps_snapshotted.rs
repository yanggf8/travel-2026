// `travel mark-maps-snapshotted <plan_id>` — record that the dashboard's static
// map PNGs were just (re)snapshotted for this plan.
//
// The dashboard map images are STATIC PNGs captured by scripts/snapshot-maps.sh
// (chromeport → R2) from the plan's POI coordinates at capture time. When the
// itinerary later changes, those PNGs silently go stale. This command stamps a
// `plan_map_snapshots.snapshotted_at` row; the `check-maps-fresh` lint compares
// it against the latest itinerary edit to flag staleness.
//
// Behavior:
//   - FAIL LOUD (non-zero) if the plan_id is not in `plans` (never stamp a
//     phantom plan).
//   - Upsert: INSERT ... ON CONFLICT(plan_id) DO UPDATE SET
//     snapshotted_at = datetime('now').
//
// Audit decision: this records the state of an EXTERNAL build artifact (the R2
// map PNGs), not a mutation of plan/itinerary domain content. It changes no
// `days`/`activities`/offer/booking row, so it intentionally does NOT bump
// plans.version, write an operation_runs row, or emit plan_events — mirroring
// the lightweight side-table convention (like the share-token write). It is a
// bookkeeping stamp; over-auditing it would pollute the audit trail with rows
// that have no domain effect.

use libsql::Connection;

pub async fn run(args: &[String], _resolved_plan_id: String) -> Result<(), String> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print_usage();
        return Ok(());
    }

    let plan_id = parse_args(args)?;

    let conn = crate::db::connect_write()
        .await
        .map_err(|e| format!("failed to connect to Turso (write tier): {e}"))?;

    // FAIL LOUD if the plan does not exist.
    if !plan_exists(&conn, &plan_id).await? {
        eprintln!(
            "Error: plan '{plan_id}' not found in plans — refusing to stamp a map snapshot."
        );
        eprintln!("       (run `travel plans` to list existing plans)");
        std::process::exit(1);
    }

    conn.execute(
        "INSERT INTO plan_map_snapshots (plan_id, snapshotted_at) \
         VALUES (?1, datetime('now')) \
         ON CONFLICT(plan_id) DO UPDATE SET snapshotted_at = datetime('now')",
        libsql::params![plan_id.clone()],
    )
    .await
    .map_err(|e| format!("plan_map_snapshots upsert failed: {e}"))?;

    // Read back the stamped value for a confirming plain-text line.
    let stamped = read_snapshotted_at(&conn, &plan_id)
        .await?
        .unwrap_or_else(|| "(now)".to_string());

    println!("marked maps snapshotted for {plan_id} at {stamped} (UTC)");
    Ok(())
}

fn parse_args(args: &[String]) -> Result<String, String> {
    let mut plan_id: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            // Accept the flag form too (the resolver consumes it upstream, but
            // this command takes the plan_id positionally).
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
    plan_id.ok_or_else(|| {
        "Error: mark-maps-snapshotted requires a <plan_id>\n\
         Example: travel mark-maps-snapshotted okinawa-2026"
            .to_string()
    })
}

fn print_usage() {
    println!(
        "Usage:\n  travel mark-maps-snapshotted <plan_id>\n\n\
         Stamps plan_map_snapshots.snapshotted_at = now for the plan, recording that the\n\
         dashboard's static map PNGs were just (re)snapshotted. The `check-maps-fresh` lint\n\
         compares this stamp against the latest itinerary edit to flag stale maps.\n\
         Intended to be called at the end of scripts/snapshot-maps.sh."
    );
}

async fn plan_exists(conn: &Connection, plan_id: &str) -> Result<bool, String> {
    let mut rows = conn
        .query(
            "SELECT 1 FROM plans WHERE plan_id = ?1",
            libsql::params![plan_id.to_string()],
        )
        .await
        .map_err(|e| format!("plans query failed: {e}"))?;
    Ok(rows
        .next()
        .await
        .map_err(|e| format!("plans row read failed: {e}"))?
        .is_some())
}

async fn read_snapshotted_at(
    conn: &Connection,
    plan_id: &str,
) -> Result<Option<String>, String> {
    let mut rows = conn
        .query(
            "SELECT snapshotted_at FROM plan_map_snapshots WHERE plan_id = ?1",
            libsql::params![plan_id.to_string()],
        )
        .await
        .map_err(|e| format!("plan_map_snapshots query failed: {e}"))?;
    if let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("plan_map_snapshots row read failed: {e}"))?
    {
        return Ok(row.get::<String>(0).ok());
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_positional_plan_id() {
        let p = parse_args(&["okinawa-2026".to_string()]).unwrap();
        assert_eq!(p, "okinawa-2026");
    }

    #[test]
    fn parse_plan_id_flag() {
        let p = parse_args(&["--plan-id".to_string(), "abc".to_string()]).unwrap();
        assert_eq!(p, "abc");
    }

    #[test]
    fn parse_missing_plan_id_errors() {
        assert!(parse_args(&[]).is_err());
    }

    #[test]
    fn parse_unknown_flag_errors() {
        assert!(parse_args(&["x".to_string(), "--bogus".to_string()]).is_err());
    }
}
