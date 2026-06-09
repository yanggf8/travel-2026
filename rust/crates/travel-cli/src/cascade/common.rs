// Shared cascade-write helpers.
//
// These were originally private to `date_change.rs`. They are the
// reusable primitives every cascade/mutation write needs: read the
// monotonic plan version, compute the next event sort_order in the
// correct bucket, insert a `plan_events` row + its `plan_event_data`
// KV child rows, and produce the volatile timestamps / run_id.
//
// The sort_order rule (mirrors the TS in-memory append behavior):
//   - dest_process events:  per-(destination, process_id) bucket,
//     assign MAX(existing) + 1 in emission order.
//   - timeline events:  GLOBAL timeline array index — assign
//     MAX(existing over ALL timeline rows) + 1 in emission order.
//     TS shares one in-memory array for the timeline, so its index is
//     global, not per-process.
//
// Volatile fields (any diff against the TS path normalizes these out):
//   - updated_at on every row
//   - operation_runs.run_id / started_at / completed_at
//   - plan_events.event_at

use libsql::Connection;

/// Read the current `plans.version` for a plan. THROWS if the plan row
/// is missing — there is no local-data fallback.
pub async fn read_version(conn: &Connection, plan_id: &str) -> Result<i64, String> {
    let mut rows = conn
        .query(
            "SELECT version FROM plans WHERE plan_id = ?1",
            libsql::params![plan_id.to_string()],
        )
        .await
        .map_err(|e| e.to_string())?;
    if let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
        let v: i64 = row.get(0).map_err(|e| e.to_string())?;
        return Ok(v);
    }
    Err(format!("plans row missing for plan_id={plan_id}"))
}

/// Compute the next sort_order for a new event in a (scope='dest_process',
/// destination, process_id) bucket. Returns `MAX(existing) + 1`.
pub async fn next_dest_process_sort_order(
    conn: &Connection,
    plan_id: &str,
    dest: &str,
    process_id: &str,
) -> Result<i64, String> {
    let mut rows = conn
        .query(
            "SELECT COALESCE(MAX(sort_order), -1) AS m FROM plan_events \
             WHERE plan_id = ?1 AND scope = 'dest_process' \
               AND destination = ?2 AND process_id = ?3",
            libsql::params![
                plan_id.to_string(),
                dest.to_string(),
                process_id.to_string()
            ],
        )
        .await
        .map_err(|e| e.to_string())?;
    if let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
        let m: i64 = row.get(0).map_err(|e| e.to_string())?;
        return Ok(m + 1);
    }
    Ok(0)
}

/// Compute the next sort_order for a new event in the GLOBAL timeline
/// bucket. Timeline events share one index across all process_ids.
/// Returns `MAX(existing) + 1`.
pub async fn next_timeline_sort_order(conn: &Connection, plan_id: &str) -> Result<i64, String> {
    let mut rows = conn
        .query(
            "SELECT COALESCE(MAX(sort_order), -1) AS m FROM plan_events \
             WHERE plan_id = ?1 AND scope = 'timeline'",
            libsql::params![plan_id.to_string()],
        )
        .await
        .map_err(|e| e.to_string())?;
    if let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
        let m: i64 = row.get(0).map_err(|e| e.to_string())?;
        return Ok(m + 1);
    }
    Ok(0)
}

/// Insert one `plan_events` row. `from_state` / `to_state` are optional
/// (NULL for non-status events like `offer_selected` / `cascade_populated`).
/// DELETE-then-reinsert is defensive (mirrors syncNormalizedTables).
#[allow(clippy::too_many_arguments)]
pub async fn insert_event(
    conn: &Connection,
    plan_id: &str,
    scope: &str,
    destination: &str,
    process_id: &str,
    sort_order: i64,
    event: &str,
    event_at: &str,
    from_state: Option<&str>,
    to_state: Option<&str>,
) -> Result<(), String> {
    conn.execute(
        "DELETE FROM plan_events \
         WHERE plan_id = ?1 AND scope = ?2 AND destination = ?3 \
           AND process_id = ?4 AND sort_order = ?5",
        libsql::params![
            plan_id.to_string(),
            scope.to_string(),
            destination.to_string(),
            process_id.to_string(),
            sort_order
        ],
    )
    .await
    .map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO plan_events \
            (plan_id, scope, destination, process_id, sort_order, \
             event, event_at, from_state, to_state) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        libsql::params![
            plan_id.to_string(),
            scope.to_string(),
            destination.to_string(),
            process_id.to_string(),
            sort_order,
            event.to_string(),
            event_at.to_string(),
            from_state.map(|s| s.to_string()),
            to_state.map(|s| s.to_string())
        ],
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Insert the `plan_event_data` KV child rows for one event.
/// DELETE-then-reinsert (defensive — mirrors syncNormalizedTables).
pub async fn insert_kv_rows(
    conn: &Connection,
    plan_id: &str,
    scope: &str,
    destination: &str,
    process_id: &str,
    sort_order: i64,
    kv: &[(&str, String)],
) -> Result<(), String> {
    conn.execute(
        "DELETE FROM plan_event_data \
         WHERE plan_id = ?1 AND scope = ?2 AND destination = ?3 \
           AND process_id = ?4 AND sort_order = ?5",
        libsql::params![
            plan_id.to_string(),
            scope.to_string(),
            destination.to_string(),
            process_id.to_string(),
            sort_order
        ],
    )
    .await
    .map_err(|e| e.to_string())?;
    for (k, v) in kv {
        conn.execute(
            "INSERT INTO plan_event_data \
                (plan_id, scope, destination, process_id, sort_order, key, value) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            libsql::params![
                plan_id.to_string(),
                scope.to_string(),
                destination.to_string(),
                process_id.to_string(),
                sort_order,
                k.to_string(),
                v.clone()
            ],
        )
        .await
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// ISO-8601 / RFC 3339 timestamp for `event_at` / `selected_at` /
/// `last_changed` (the TS path uses `new Date().toISOString()`).
pub fn now_rfc3339() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let dur = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = dur.as_secs() as i64;
    let nanos = dur.subsec_nanos();
    let (year, month, day, hour, min, sec) = civil_from_unix(secs);
    format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{min:02}:{sec:02}.{:03}Z",
        nanos / 1_000_000
    )
}

/// SQLite-friendly `YYYY-MM-DD HH:MM:SS` UTC timestamp (matches
/// `datetime('now')`, computed in Rust for one consistent value).
pub fn now_db_datetime() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let (year, month, day, hour, min, sec) = civil_from_unix(secs);
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{min:02}:{sec:02}")
}

/// Howard Hinnant's civil-from-days algorithm. Returns
/// (year, month, day, hour, min, sec) for a unix epoch in seconds.
pub fn civil_from_unix(secs: i64) -> (i32, u32, u32, u32, u32, u32) {
    let days = secs.div_euclid(86_400);
    let secs_of_day = secs.rem_euclid(86_400) as u32;
    let hour = secs_of_day / 3600;
    let min = (secs_of_day % 3600) / 60;
    let sec = secs_of_day % 60;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m as u32, d as u32, hour, min, sec)
}

/// Generate a fresh `operation_runs.run_id` (UUIDv4-shaped). The value is
/// VOLATILE — diffs normalize run_id out.
pub fn new_run_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
        ^ (n as u128);
    let p1 = (nanos & 0xFFFF_FFFF) as u32;
    let p2 = ((nanos >> 32) & 0xFFFF) as u16;
    let p3 = ((nanos >> 48) & 0x0FFF) as u16;
    let p4 = 0x8000 | (((nanos >> 60) & 0x3FFF) as u16);
    let p5 = (nanos as u64) ^ 0xDEAD_BEEF_CAFE_F00D;
    format!("{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}", p1, p2, p3, p4, p5)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_from_unix_epoch() {
        assert_eq!(civil_from_unix(0), (1970, 1, 1, 0, 0, 0));
        assert_eq!(civil_from_unix(1_771_329_600), (2026, 2, 17, 12, 0, 0));
        assert_eq!(civil_from_unix(1_780_876_800), (2026, 6, 8, 0, 0, 0));
    }

    #[test]
    fn rfc3339_shape() {
        let s = now_rfc3339();
        assert!(s.len() >= 24, "got: {s}");
        let b = s.as_bytes();
        assert_eq!(b[4], b'-');
        assert_eq!(b[7], b'-');
        assert_eq!(b[10], b'T');
        assert_eq!(b[13], b':');
        assert_eq!(b[16], b':');
        assert_eq!(b[19], b'.');
        assert_eq!(b[s.len() - 1], b'Z');
    }
}
