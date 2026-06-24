// `travel check-maps-fresh [--plan-id <id>]` — lint: flag plans whose dashboard
// map PNGs have gone stale relative to the itinerary, and whose map-artifact
// manifest is incomplete (MISSING/EMPTY keys).
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
// Completeness (manifest-based):
//   - `map_artifacts` rows are written by snapshot-maps.sh (one per expected key).
//   - Expected keys: `plan.png` + `day-{n}.png` for each day in the plan.
//   - Each key is MISSING (no row), EMPTY (status != uploaded or byte_size <= 64),
//     or OK.
//
// KNOWN LIMITATION: `day_route_segments` has NO `updated_at` column, so a
// route-only edit (changing the per-segment path/legs without touching a
// day/session/activity/meal row) is INVISIBLE to this timestamp lint. Treat a
// fresh result as "no itinerary-content edit since snapshot", not a hard
// guarantee that the rendered map matches. Re-run snapshot-maps after any route
// edit regardless.
//
// This is a lint/advisory: plain-text output, exit 0 (never fails the build).

use std::collections::HashMap;

use libsql::Connection;

pub async fn run(args: &[String]) -> Result<(), String> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!(
            "Usage:\n  travel check-maps-fresh [--plan-id <id>]\n\n\
             Flags plans whose static dashboard map PNGs are stale relative to the\n\
             latest itinerary edit, and whose map-artifact manifest is incomplete\n\
             (advisory; always exits 0).\n\
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
    let mut incomplete = 0usize;
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

        match evaluate_completeness(&conn, plan_id).await? {
            CompletenessVerdict::NoManifest => {
                incomplete += 1;
                println!("⚠ {plan_id}: no map manifest — run snapshot-maps");
            }
            CompletenessVerdict::Complete { line } => {
                println!("✓ {line}");
            }
            CompletenessVerdict::Incomplete { line } => {
                incomplete += 1;
                println!("⚠ {line}");
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
    if incomplete == 0 {
        println!("Completeness: all {} plan(s) have complete map manifests.", plan_ids.len());
    } else {
        println!(
            "Completeness: {incomplete} of {} plan(s) have missing/empty map artifacts — run snapshot-maps.",
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

/// Completeness verdict for one plan's map-artifact manifest.
pub enum CompletenessVerdict {
    NoManifest,
    Complete { line: String },
    Incomplete { line: String },
}

/// One row from `map_artifacts` — just the fields the classifier needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestRow {
    pub byte_size: i64,
    pub status: String,
}

/// Per-key classification for the map-artifact manifest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactClass {
    Missing,
    Empty,
    Ok,
}

/// Build the expected map keys for a plan: `plan.png` plus `day-{n}.png` per day.
pub fn expected_map_keys(day_numbers: &[i64]) -> Vec<String> {
    let mut keys = vec!["plan.png".to_string()];
    let mut sorted = day_numbers.to_vec();
    sorted.sort_unstable();
    for n in sorted {
        keys.push(format!("day-{n}.png"));
    }
    keys
}

/// Classify one expected key against the manifest (pure — unit-testable).
pub fn classify_artifact(manifest_row: Option<&ManifestRow>) -> ArtifactClass {
    match manifest_row {
        None => ArtifactClass::Missing,
        Some(row) => {
            if row.status != "uploaded" || row.byte_size <= 64 {
                ArtifactClass::Empty
            } else {
                ArtifactClass::Ok
            }
        }
    }
}

/// Format the per-plan completeness line (pure — unit-testable).
pub fn format_completeness_line(
    plan_id: &str,
    expected: &[String],
    manifest: &HashMap<String, ManifestRow>,
) -> String {
    let total = expected.len();
    let mut ok_count = 0usize;
    let mut missing = Vec::new();
    let mut empty = Vec::new();

    for key in expected {
        match classify_artifact(manifest.get(key)) {
            ArtifactClass::Ok => ok_count += 1,
            ArtifactClass::Missing => missing.push(key.as_str()),
            ArtifactClass::Empty => empty.push(key.as_str()),
        }
    }

    if missing.is_empty() && empty.is_empty() {
        return format!("{plan_id}: maps {ok_count}/{total} ok");
    }

    let mut parts = vec![format!("{plan_id}: maps {ok_count}/{total} ok")];
    if !missing.is_empty() {
        parts.push(format!("MISSING: {}", missing.join(", ")));
    }
    if !empty.is_empty() {
        parts.push(format!(
            "EMPTY: {} (run snapshot-maps)",
            empty.join(", ")
        ));
    }
    parts.join(" — ")
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

/// Evaluate one plan's map-artifact manifest completeness. Reusable by `doctor`.
pub async fn evaluate_completeness(
    conn: &Connection,
    plan_id: &str,
) -> Result<CompletenessVerdict, String> {
    let day_numbers = read_day_numbers(conn, plan_id).await?;
    let expected = expected_map_keys(&day_numbers);
    let manifest = read_map_artifacts(conn, plan_id).await?;

    if manifest.is_empty() {
        return Ok(CompletenessVerdict::NoManifest);
    }

    let line = format_completeness_line(plan_id, &expected, &manifest);
    let all_ok = expected.iter().all(|k| {
        classify_artifact(manifest.get(k)) == ArtifactClass::Ok
    });

    if all_ok {
        Ok(CompletenessVerdict::Complete { line })
    } else {
        Ok(CompletenessVerdict::Incomplete { line })
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

async fn read_day_numbers(conn: &Connection, plan_id: &str) -> Result<Vec<i64>, String> {
    let mut rows = conn
        .query(
            "SELECT day_number FROM days WHERE plan_id = ?1 ORDER BY day_number",
            libsql::params![plan_id.to_string()],
        )
        .await
        .map_err(|e| format!("days query failed: {e}"))?;
    let mut out = Vec::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("days row read failed: {e}"))?
    {
        if let Ok(n) = row.get::<i64>(0) {
            out.push(n);
        }
    }
    Ok(out)
}

async fn read_map_artifacts(
    conn: &Connection,
    plan_id: &str,
) -> Result<HashMap<String, ManifestRow>, String> {
    let mut rows = match conn
        .query(
            "SELECT map_key, byte_size, status FROM map_artifacts WHERE plan_id = ?1",
            libsql::params![plan_id.to_string()],
        )
        .await
    {
        Ok(r) => r,
        // Table not migrated yet → treat as empty manifest (advisory lint, never crash).
        Err(e) if e.to_string().contains("no such table: map_artifacts") => {
            return Ok(HashMap::new());
        }
        Err(e) => return Err(format!("map_artifacts query failed: {e}")),
    };
    let mut out = HashMap::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("map_artifacts row read failed: {e}"))?
    {
        let key = row
            .get::<String>(0)
            .map_err(|e| format!("map_key read failed: {e}"))?;
        let byte_size = row.get::<i64>(1).unwrap_or(0);
        let status = row.get::<String>(2).unwrap_or_default();
        out.insert(key, ManifestRow { byte_size, status });
    }
    Ok(out)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(entries: &[(&str, i64, &str)]) -> HashMap<String, ManifestRow> {
        entries
            .iter()
            .map(|(k, size, status)| {
                (
                    (*k).to_string(),
                    ManifestRow {
                        byte_size: *size,
                        status: (*status).to_string(),
                    },
                )
            })
            .collect()
    }

    #[test]
    fn expected_map_keys_includes_plan_and_days() {
        assert_eq!(
            expected_map_keys(&[3, 1, 2]),
            vec![
                "plan.png".to_string(),
                "day-1.png".to_string(),
                "day-2.png".to_string(),
                "day-3.png".to_string(),
            ]
        );
    }

    #[test]
    fn classify_missing_empty_ok() {
        assert_eq!(classify_artifact(None), ArtifactClass::Missing);
        assert_eq!(
            classify_artifact(Some(&ManifestRow {
                byte_size: 100,
                status: "failed".into(),
            })),
            ArtifactClass::Empty
        );
        assert_eq!(
            classify_artifact(Some(&ManifestRow {
                byte_size: 1,
                status: "uploaded".into(),
            })),
            ArtifactClass::Empty
        );
        assert_eq!(
            classify_artifact(Some(&ManifestRow {
                byte_size: 65,
                status: "uploaded".into(),
            })),
            ArtifactClass::Ok
        );
    }

    #[test]
    fn format_completeness_line_okinawa_example() {
        let expected = expected_map_keys(&[1, 2, 3, 4, 5]);
        let m = manifest(&[
            ("plan.png", 1, "uploaded"),
            ("day-2.png", 5000, "uploaded"),
            ("day-3.png", 4000, "uploaded"),
        ]);
        let line = format_completeness_line("okinawa-2026", &expected, &m);
        assert!(line.contains("okinawa-2026: maps 2/6 ok"));
        assert!(line.contains("MISSING: day-1.png, day-4.png, day-5.png"));
        assert!(line.contains("EMPTY: plan.png (run snapshot-maps)"));
    }

    #[test]
    fn format_completeness_line_all_ok() {
        let expected = expected_map_keys(&[1, 2]);
        let m = manifest(&[
            ("plan.png", 1000, "uploaded"),
            ("day-1.png", 2000, "uploaded"),
            ("day-2.png", 3000, "uploaded"),
        ]);
        let line = format_completeness_line("tokyo-2026", &expected, &m);
        assert_eq!(line, "tokyo-2026: maps 3/3 ok");
    }
}