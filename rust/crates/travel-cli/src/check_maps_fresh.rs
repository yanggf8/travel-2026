// `travel check-maps-fresh [--plan-id <id>]` — lint: flag plans whose dashboard
// map PNGs have gone stale relative to the itinerary.
//
// The dashboard's map images are STATIC PNG snapshots (captured by
// scripts/snapshot-maps.sh via chromeport → R2) baked from the plan's POI
// coordinates at capture time. When the itinerary changes (activities
// added/moved/removed, days changed, meals edited), those PNGs silently no
// longer match the day's stops — same silent-drift class we've been killing.
//
// Timestamp-based staleness (user-chosen design):
//   - plan_map_snapshots.snapshotted_at records the last snapshot time
//     (stamped by `mark-maps-snapshotted`, called at the end of snapshot-maps.sh).
//   - We take MAX(updated_at) across the four itinerary tables that carry an
//     `updated_at` column — `days`, `timesofday`, `activities`, `session_meals` —
//     for the plan. If the latest itinerary edit is newer than the snapshot,
//     the maps are STALE.
//
// KNOWN LIMITATION: `day_route_segments` has NO `updated_at` column, so a
// route-only edit (changing the per-segment path/legs without touching a
// day/session/activity/meal row) is INVISIBLE to this timestamp lint. Treat a
// fresh result as "no itinerary-content edit since snapshot", not a hard
// guarantee that the rendered map matches. Re-run snapshot-maps after any route
// edit regardless.
//
// This is a lint/advisory: plain-text output, exit 0 (never fails the build).

use libsql::Connection;

pub async fn run(args: &[String]) -> Result<(), String> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!(
            "Usage:\n  travel check-maps-fresh [--plan-id <id>]\n\n\
             Flags plans whose static dashboard map PNGs are stale relative to the\n\
             latest itinerary edit (advisory; always exits 0).\n\
             With --plan-id, checks that one plan; otherwise checks all live plans."
        );
        return Ok(());
    }

    let conn = crate::db::connect_read()
        .await
        .map_err(|e| format!("failed to connect to Turso (read tier): {e}"))?;

    // Resolve scope: a specific plan if one was given, else every live plan.
    let plan_ids = if has_plan_selector(args) {
        let plan_id = crate::plan_resolver::resolve_plan_id(args).await?;
        vec![plan_id]
    } else {
        all_live_plans(&conn).await?
    };

    if plan_ids.is_empty() {
        println!("No plans to check.");
        return Ok(());
    }

    let mut stale = 0usize;
    for plan_id in &plan_ids {
        match evaluate(&conn, plan_id).await? {
            Status::NeverSnapshotted => {
                stale += 1;
                println!(
                    "⚠ {plan_id}: maps never snapshotted — run scripts/snapshot-maps.sh {plan_id} <dest>"
                );
            }
            Status::Stale { snapshotted_at } => {
                stale += 1;
                println!(
                    "⚠ {plan_id}: itinerary changed since maps snapshotted ({snapshotted_at}) — maps STALE, re-run scripts/snapshot-maps.sh"
                );
            }
            Status::Fresh { snapshotted_at } => {
                println!("✓ {plan_id}: maps fresh (snapshotted {snapshotted_at})");
            }
        }
    }

    println!();
    if stale == 0 {
        println!("Summary: all {} plan(s) have fresh maps.", plan_ids.len());
    } else {
        println!(
            "Summary: {stale} of {} plan(s) have stale maps — re-run scripts/snapshot-maps.sh.",
            plan_ids.len()
        );
    }
    println!(
        "Note: route-only edits (day_route_segments has no updated_at) are invisible to this \
         timestamp lint; re-snapshot after any route edit regardless."
    );

    // Advisory lint: never fail the build.
    Ok(())
}

/// Freshness verdict for one plan.
pub enum Status {
    NeverSnapshotted,
    Stale { snapshotted_at: String },
    Fresh { snapshotted_at: String },
}

/// Evaluate one plan's map-snapshot freshness. Reusable by `doctor`.
pub async fn evaluate(conn: &Connection, plan_id: &str) -> Result<Status, String> {
    let snapshotted_at = read_snapshotted_at(conn, plan_id).await?;
    let Some(snapshotted_at) = snapshotted_at else {
        return Ok(Status::NeverSnapshotted);
    };

    let max_edit = max_itinerary_updated_at(conn, plan_id).await?;
    // Lexical compare is correct for `YYYY-MM-DD HH:MM:SS` (datetime('now')).
    let stale = match &max_edit {
        Some(edit) => edit.as_str() > snapshotted_at.as_str(),
        None => false, // no itinerary rows → nothing could have drifted
    };

    if stale {
        Ok(Status::Stale { snapshotted_at })
    } else {
        Ok(Status::Fresh { snapshotted_at })
    }
}

/// True if the caller named a specific plan (a plan-selecting flag, or any bare
/// positional argument). When false, `run` checks every live plan.
fn has_plan_selector(args: &[String]) -> bool {
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        match a.as_str() {
            "--plan-id" | "--travel-date" | "--travel-start" | "--travel-end" => return true,
            // skip an unknown flag's value too, conservatively
            other if other.starts_with("--") => i += 1,
            // a bare positional token = an explicit plan id
            _ => return true,
        }
    }
    false
}

async fn all_live_plans(conn: &Connection) -> Result<Vec<String>, String> {
    let mut rows = conn
        .query(
            "SELECT plan_id FROM plans WHERE deleted_at IS NULL ORDER BY plan_id",
            (),
        )
        .await
        .map_err(|e| format!("plans query failed: {e}"))?;
    let mut out = Vec::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("plans row read failed: {e}"))?
    {
        if let Ok(id) = row.get::<String>(0) {
            out.push(id);
        }
    }
    Ok(out)
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
        return Ok(row.get::<String>(0).ok().filter(|s| !s.is_empty()));
    }
    Ok(None)
}

/// MAX(updated_at) across the four itinerary tables that carry an `updated_at`
/// column. A single UNION ALL of per-table maxima, then the overall max.
async fn max_itinerary_updated_at(
    conn: &Connection,
    plan_id: &str,
) -> Result<Option<String>, String> {
    let sql = "SELECT MAX(u) FROM (\
        SELECT MAX(updated_at) AS u FROM days          WHERE plan_id = ?1 \
        UNION ALL \
        SELECT MAX(updated_at) AS u FROM timesofday    WHERE plan_id = ?1 \
        UNION ALL \
        SELECT MAX(updated_at) AS u FROM activities     WHERE plan_id = ?1 \
        UNION ALL \
        SELECT MAX(updated_at) AS u FROM session_meals  WHERE plan_id = ?1 \
    )";
    let mut rows = conn
        .query(sql, libsql::params![plan_id.to_string()])
        .await
        .map_err(|e| format!("itinerary updated_at query failed: {e}"))?;
    if let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("itinerary updated_at row read failed: {e}"))?
    {
        return Ok(row.get::<String>(0).ok().filter(|s| !s.is_empty()));
    }
    Ok(None)
}
