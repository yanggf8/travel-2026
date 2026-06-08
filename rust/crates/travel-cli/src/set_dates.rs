// `travel set-dates <start> <end> [reason]` — port of
// src/cli/commands/set-dates.ts. WRITE-capable.
//
// Validates the date range, prints the human-friendly header, then
// calls into `cascade::date_change::execute` for the actual DB write.
// The cascade module owns the full set-dates side-effect:
//   - UPDATE date_anchors
//   - UPDATE cascade_dirty_flags (4 date-dependent processes on the
//     active destination)
//   - INSERT plan_events + plan_event_data (date_anchor_changed +
//     4 marked_dirty on dest_process; date_anchor_changed +
//     marked_global_dirty + 4 marked_dirty on timeline)
//   - INSERT operation_runs (audit row, set-dates / completed)
//   - UPDATE plans.version (+1, atomic)
//
// `--dry-run` is a no-write mode: print the header + "DRY RUN" line
// and exit. (Not currently exposed via clap, but the flag is reserved
// for parity with the TS CLI's `--dry-run`.)

use crate::status::format_date;
use crate::validate::validate_date_range;
use libsql::Connection;

/// Run set-dates mutation.
///
/// `plan_id` is the resolved plan identifier (from `$TRAVEL_PLAN_ID` or
/// explicit `--plan-id`). The function returns `Err` if the validation
/// fails, the DB write fails, or the plan is missing the `plans` row.
/// No local-data fallback — Turso is the SOLE source of truth.
pub async fn run(
    start: String,
    end: String,
    reason: Option<String>,
    plan_id: String,
) -> Result<(), String> {
    // 1. Validate (byte-parity error messages with TS — see validate.rs).
    let days = match validate_date_range(&start, &end) {
        Ok(d) => d as i64,
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    };

    // 2. Print the header. Mirrors the TS console.log lines:
    //      \n📅 Setting dates: <start_fmt> → <end_fmt> (<N> days)
    //      Reason: <reason>           (only if reason non-empty)
    let start_fmt = format_date(&start);
    let end_fmt = format_date(&end);
    println!("\n📅 Setting dates: {} → {} ({} days)", start_fmt, end_fmt, days);
    if let Some(r) = &reason {
        println!("   Reason: {r}");
    }

    // 3. Connect write-tier (token resolution goes through
    //    turso-util → /home/yanggf/b/gwebcdb/crates/turso-util).
    let conn = match db_connect_write().await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error: failed to connect to Turso (write tier): {e}");
            std::process::exit(1);
        }
    };

    // 4. Read the active destination from plan_metadata. The cascade
    //    only writes the active dest's date_anchors + dirty flags.
    let active_dest = match read_active_destination(&conn, &plan_id).await {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error: failed to resolve active destination: {e}");
            std::process::exit(1);
        }
    };

    // 5. Read the previous date_anchors row (for the event payload's
    //    `from_dates` field). THROWS if the row is missing — no
    //    local-data fallback.
    let (old_start, old_end) = match read_date_anchor(&conn, &plan_id, &active_dest).await {
        Ok((s, e)) => (s, e),
        Err(e) => {
            eprintln!("Error: failed to read current date anchor: {e}");
            std::process::exit(1);
        }
    };

    // 6. Execute the cascade write. The cascade is the SINGLE place
    //    that touches the DB for set-dates — no half-applied state.
    match crate::cascade::date_change::execute(
        &conn,
        &plan_id,
        &active_dest,
        &start,
        &end,
        days,
        reason.as_deref(),
        old_start.as_deref(),
        old_end.as_deref(),
    )
    .await
    {
        Ok(_version_after) => {
            println!("✅ Dates updated and cascade triggered");
            Ok(())
        }
        Err(e) => {
            eprintln!("Error: set-dates cascade failed: {e}");
            std::process::exit(1);
        }
    }
}

async fn db_connect_write() -> Result<Connection, String> {
    crate::db::connect_write().await
}

async fn read_active_destination(
    conn: &Connection,
    plan_id: &str,
) -> Result<String, String> {
    let mut rows = conn
        .query(
            "SELECT active_destination FROM plan_metadata WHERE plan_id = ?1",
            libsql::params![plan_id.to_string()],
        )
        .await
        .map_err(|e| format!("plan_metadata query failed: {e}"))?;
    if let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("plan_metadata row read failed: {e}"))?
    {
        let dest: String = row
            .get(0)
            .map_err(|e| format!("active_destination col read failed: {e}"))?;
        if dest.is_empty() {
            return Err(format!(
                "plan_metadata.active_destination is empty for plan_id={plan_id}"
            ));
        }
        return Ok(dest);
    }
    Err(format!(
        "plan_metadata row missing for plan_id={plan_id} (no local-data fallback)"
    ))
}

async fn read_date_anchor(
    conn: &Connection,
    plan_id: &str,
    dest: &str,
) -> Result<(Option<String>, Option<String>), String> {
    let mut rows = conn
        .query(
            "SELECT start_date, end_date FROM date_anchors \
             WHERE plan_id = ?1 AND destination = ?2",
            libsql::params![plan_id.to_string(), dest.to_string()],
        )
        .await
        .map_err(|e| format!("date_anchors query failed: {e}"))?;
    if let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("date_anchors row read failed: {e}"))?
    {
        let s: String = row.get(0).unwrap_or_default();
        let e: String = row.get(1).unwrap_or_default();
        return Ok((
            if s.is_empty() { None } else { Some(s) },
            if e.is_empty() { None } else { Some(e) },
        ));
    }
    // No prior anchor — first-time set. Event payload uses "null" for
    // from_dates, matching the TS path's `oldStart ? ... : null` branch.
    Ok((None, None))
}
