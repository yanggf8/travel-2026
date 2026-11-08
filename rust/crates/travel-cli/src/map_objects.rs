// Shared R2 map-object purge. One home for deleting a plan's map PNG from the MAPS bucket and
// the manifest rows that describe it (`map_artifacts`, `map_legend_stops`), used by
// snapshot-maps (stale map removal) and the day-variant commands (day_number reuse).
//
// Test seam: `TRAVEL_MAPS_DRY_DELETE=1` prints `would delete <bucket>/<plan>/<key>` instead of
// running Wrangler, so tests never touch real R2.

use libsql::params;

use crate::snapshot_maps::{wrangler_command, BUCKET};

/// Delete `key` from R2 when its `map_artifacts` row says `uploaded`, then drop that key's
/// `map_artifacts` and `map_legend_stops` rows. Returns `Ok(true)` when an R2 object was
/// (or, under `TRAVEL_MAPS_DRY_DELETE`, would have been) deleted. An R2 failure returns `Err`
/// BEFORE any row is touched, so the caller can abort with nothing deleted.
pub async fn purge_map_key(plan: &str, key: &str, reason: &str) -> Result<bool, String> {
    // Own connection: callers run this after long Wrangler uploads, when a long-lived
    // connection may have idled out.
    let c = crate::db::connect_write().await?;
    let mut r = c
        .query(
            "SELECT status FROM map_artifacts WHERE plan_id = ?1 AND map_key = ?2",
            params![plan.to_string(), key.to_string()],
        )
        .await
        .map_err(|e| format!("map manifest read: {e}"))?;
    let was_uploaded = match r.next().await.map_err(|e| format!("map manifest row: {e}"))? {
        Some(row) => row.get::<Option<String>>(0).ok().flatten().as_deref() == Some("uploaded"),
        None => false,
    };
    if was_uploaded {
        let target = format!("{BUCKET}/{plan}/{key}");
        if std::env::var("TRAVEL_MAPS_DRY_DELETE").as_deref() == Ok("1") {
            println!("would delete {target}");
        } else {
            let status = wrangler_command()
                .args(["wrangler", "r2", "object", "delete", &target, "--remote"])
                .status()
                .map_err(|e| format!("failed to run Wrangler delete for {key}: {e}"))?;
            if !status.success() {
                return Err(format!(
                    "Wrangler could not delete {key} ({reason}) (exit {status})"
                ));
            }
        }
    }
    c.execute(
        "DELETE FROM map_legend_stops WHERE plan_id = ?1 AND map_key = ?2",
        params![plan.to_string(), key.to_string()],
    )
    .await
    .map_err(|e| format!("map_legend_stops delete failed: {e}"))?;
    c.execute(
        "DELETE FROM map_artifacts WHERE plan_id = ?1 AND map_key = ?2",
        params![plan.to_string(), key.to_string()],
    )
    .await
    .map_err(|e| format!("map_artifacts delete failed: {e}"))?;
    Ok(was_uploaded)
}

/// The three per-day map keys a day number can own.
pub fn day_map_keys(day: i64) -> [String; 3] {
    [
        format!("day-{day}.png"),
        format!("day-{day}-logistics.png"),
        format!("day-{day}-away.png"),
    ]
}

/// Purge every `day-{n}*.png` object + manifest row for one day number, so a number handed out
/// again (variant create/remove) never inherits an old map.
pub async fn purge_day_maps(plan: &str, day: i64) -> Result<(), String> {
    for key in day_map_keys(day) {
        purge_map_key(plan, &key, "day number reuse").await?;
    }
    Ok(())
}
