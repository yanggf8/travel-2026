// `travel set-airport-transfer <arrival|departure> <planned|booked>
//   --selected "<title|route|duration|price|schedule>"
//   [--candidate "<...>"]...` — port of
// src/cli/commands/set-airport-transfer.ts. NO CASCADE.
//
// Mirrors the TS path:
//   1. UPDATE airport_transfers for the given direction: set the
//      selected_* columns from the parsed --selected spec
//   2. DELETE-then-reinsert airport_transfer_candidates for this
//      direction (the candidate list comes from --candidate args)
//   3. INSERT 1 dest_process + 1 timeline plan_event
//      (airport_transfer_updated, process_3_transportation) with
//      KV payload = {direction, status, selected_id, candidates_count}
//   4. INSERT operation_runs + UPDATE plans.version
//
// Verified no-cascade: cascade_dirty_flags UNCHANGED.

use libsql::Connection;

#[derive(Default, Debug, Clone)]
struct TransferOption {
    title: String,
    route: String,
    duration_min: Option<i64>,
    price_yen: Option<i64>,
    schedule: Option<String>,
    id: String,
}

pub async fn run(
    args: &[String],
    plan_id: String,
) -> Result<(), String> {
    if args.len() < 2 {
        eprintln!("Error: set-airport-transfer requires <arrival|departure> <planned|booked>");
        eprintln!("Example: set-airport-transfer arrival planned --selected \"Limousine Bus|NRT T1 → Shiodome|85|3200|19:40 → ~21:05\"");
        std::process::exit(1);
    }
    let direction = args[0].clone();
    if direction != "arrival" && direction != "departure" {
        eprintln!("Error: <arrival|departure> must be one of: arrival | departure");
        std::process::exit(1);
    }
    let status = args[1].clone();
    if status != "planned" && status != "booked" {
        eprintln!("Error: <planned|booked> must be one of: planned | booked");
        std::process::exit(1);
    }

    let parsed = parse_args(&args[2..])?;
    let selected_opt = match parsed.selected {
        Some(s) => s,
        None => {
            eprintln!("Error: set-airport-transfer requires --selected \"<title|route|...>\"");
            std::process::exit(1);
        }
    };
    // Re-parse the selected/candidate specs with the real direction
    // (the id is direction-prefixed). The raw spec string was stored
    // during parse_args; re-call parse_transfer_spec with the real
    // direction here.
    let selected_opt = reparse_with_direction(&selected_opt, &direction);
    let candidates: Vec<TransferOption> = parsed
        .candidates
        .iter()
        .map(|c| reparse_with_direction(c, &direction))
        .collect();

    let conn = match crate::db::connect_write().await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error: failed to connect to Turso (write tier): {e}");
            std::process::exit(1);
        }
    };

    let destination = match read_destination(&conn, &plan_id, &args[2..]).await {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    };

    println!("\n🚌 Setting airport transfer:");
    println!("   Destination: {destination}");
    println!("   Direction: {direction}");
    println!("   Status: {status}");
    println!("   Selected: {}", selected_opt.title);
    if !candidates.is_empty() {
        println!("   Candidates: {}", candidates.len());
    }

    match execute(
        &conn,
        &plan_id,
        &destination,
        &direction,
        &status,
        &selected_opt,
        &candidates,
    )
    .await
    {
        Ok(_) => {
            println!("✅ Airport transfer updated");
            Ok(())
        }
        Err(e) => {
            eprintln!("Error: set-airport-transfer failed: {e}");
            std::process::exit(1);
        }
    }
}

#[derive(Default, Debug)]
struct ParsedArgs {
    selected: Option<TransferOption>,
    candidates: Vec<TransferOption>,
    selected_raw: Option<String>,
    candidate_raws: Vec<String>,
}

/// Re-parse a TransferOption with the real direction (the
/// TransferOption's title/route/duration/price/schedule are already
/// correct; only the id needs to be recomputed because it was
/// generated with a placeholder direction during the initial
/// arg-parsing pass).
fn reparse_with_direction(t: &TransferOption, direction: &str) -> TransferOption {
    let mut t2 = t.clone();
    t2.id = format!(
        "{}_{}_{}",
        direction,
        slugify(&t.title),
        hash_string(&t.route)
    );
    t2
}

fn parse_args(args: &[String]) -> Result<ParsedArgs, String> {
    let mut parsed = ParsedArgs::default();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        match a.as_str() {
            "--selected" => {
                let v = args
                    .get(i + 1)
                    .ok_or_else(|| "missing value for --selected".to_string())?;
                parsed.selected_raw = Some(v.clone());
                parsed.selected = Some(parse_transfer_spec("?selected", v));
                i += 2;
            }
            "--candidate" => {
                let v = args
                    .get(i + 1)
                    .ok_or_else(|| "missing value for --candidate".to_string())?;
                parsed.candidate_raws.push(v.clone());
                parsed
                    .candidates
                    .push(parse_transfer_spec("?candidate", v));
                i += 2;
            }
            other if other.starts_with("--") => {
                return Err(format!("unknown argument: {other}"));
            }
            _ => i += 1,
        }
    }
    Ok(parsed)
}

/// Mirror of `parseTransferSpec(direction, spec)` from
/// `src/cli/shared/transfer-parsing.ts`. Spec format:
///   "title|route|duration|price|schedule"
/// `id` is computed from `direction` + slugify(title) +
/// hashString(route)[:6]. We do NOT know the direction in the
/// caller path before we get here; we pass a placeholder
/// `direction_arg` (e.g. "?selected") and rewrite the id when we
/// know the real direction. But to keep the id identical to TS
/// we MUST know the direction. So this parser is called from
/// `execute` after the real direction is parsed — see below for
/// the version that takes the real direction.
fn parse_transfer_spec(direction: &str, spec: &str) -> TransferOption {
    let parts: Vec<&str> = spec.split('|').map(|p| p.trim()).collect();
    let title = parts.first().copied().unwrap_or("").to_string();
    let route = parts.get(1).copied().unwrap_or("").to_string();
    let duration_min: Option<i64> = parts
        .get(2)
        .and_then(|p| p.parse::<i64>().ok())
        .filter(|n| n.is_finite_abs());
    let price_yen: Option<i64> = parts
        .get(3)
        .and_then(|p| p.parse::<i64>().ok())
        .filter(|n| n.is_finite_abs());
    let schedule: Option<String> = parts
        .get(4)
        .filter(|p| !p.is_empty())
        .map(|p| p.to_string());
    let id = format!(
        "{}_{}_{}",
        direction,
        slugify(&title),
        hash_string(&route)
    );
    TransferOption {
        title,
        route,
        duration_min,
        price_yen,
        schedule,
        id,
    }
}

trait FiniteAbs {
    fn is_finite_abs(&self) -> bool;
}
impl FiniteAbs for i64 {
    fn is_finite_abs(&self) -> bool {
        // i64 is always finite; the `Number.isFinite` check in TS
        // is a no-op for ints but matters for the parseInt('abc') =
        // NaN case. We've already filtered on `.ok()` so non-numeric
        // inputs return None — this fn exists for the type shape.
        true
    }
}

/// Slugify text for use in transfer IDs.
/// Mirror of `slugify` from `src/cli/shared/transfer-parsing.ts`.
fn slugify(text: &str) -> String {
    let mut out = String::new();
    let mut last_underscore = true; // suppress leading underscore
    for c in text.chars() {
        let lc = c.to_ascii_lowercase();
        if lc.is_ascii_alphanumeric() {
            out.push(lc);
            last_underscore = false;
        } else if !last_underscore {
            out.push('_');
            last_underscore = true;
        }
    }
    // strip trailing underscore
    while out.ends_with('_') {
        out.pop();
    }
    if out.len() > 48 {
        out.truncate(48);
        while out.ends_with('_') {
            out.pop();
        }
    }
    out
}

/// djb2 hash, base36-encoded, matching `hashString` from
/// `src/cli/shared/transfer-parsing.ts`. Returns up to 6 chars to
/// match the TS `.slice(0, 6)`.
///
/// The TS implementation does `hash |= 0` after each step, which
/// truncates the 32-bit integer (`| 0` is the JS idiom for int32
/// coercion). To match byte-for-byte, we mirror that truncation
/// AFTER each multiply-add step.
fn hash_string(text: &str) -> String {
    let mut hash: i32 = 5381;
    for c in text.chars() {
        // (hash << 5) + hash  ==  hash * 33
        let h = (hash as i64).wrapping_mul(33).wrapping_add(c as i64);
        // Truncate to int32 — `| 0` in JS.
        hash = h as i32;
    }
    let abs = (hash as i64).unsigned_abs();
    let mut s = String::new();
    let mut n = abs;
    if n == 0 {
        return "0".to_string();
    }
    while n > 0 {
        let d = (n % 36) as u32;
        let c = if d < 10 { b'0' + d as u8 } else { b'a' + (d - 10) as u8 };
        s.insert(0, c as char);
        n /= 36;
    }
    if s.len() > 6 {
        s.truncate(6);
    }
    s
}

async fn read_destination(
    conn: &Connection,
    plan_id: &str,
    args: &[String],
) -> Result<String, String> {
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--dest"
            && let Some(d) = args.get(i + 1)
        {
            return Ok(d.clone());
        }
        i += 1;
    }
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
            return Err("plan_metadata.active_destination is empty".to_string());
        }
        return Ok(dest);
    }
    Err(format!(
        "plan_metadata row missing for plan_id={plan_id}"
    ))
}

async fn execute(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    direction: &str,
    status: &str,
    selected: &TransferOption,
    candidates: &[TransferOption],
) -> Result<i64, String> {
    let now_iso = now_rfc3339();
    let now_db = now_db_datetime();

    let version_before = read_version(conn, plan_id).await?;
    let version_after = version_before + 1;

    // 1. UPDATE airport_transfers (selected_* columns + status).
    conn.execute(
        "UPDATE airport_transfers SET \
            status = ?1, \
            selected_id = ?2, \
            selected_title = ?3, \
            selected_route = ?4, \
            selected_duration_min = ?5, \
            selected_price_yen = ?6, \
            selected_schedule = ?7, \
            selected_booking_url = NULL, \
            selected_notes = NULL, \
            updated_at = ?8 \
         WHERE plan_id = ?9 AND destination = ?10 AND direction = ?11",
        libsql::params![
            status.to_string(),
            selected.id.clone(),
            selected.title.clone(),
            selected.route.clone(),
            selected.duration_min,
            selected.price_yen,
            selected.schedule.clone(),
            now_db.clone(),
            plan_id.to_string(),
            destination.to_string(),
            direction.to_string()
        ],
    )
    .await
    .map_err(|e| format!("airport_transfers UPDATE failed: {e}"))?;

    // 2. DELETE-then-reinsert candidates.
    conn.execute(
        "DELETE FROM airport_transfer_candidates \
         WHERE plan_id = ?1 AND destination = ?2 AND direction = ?3",
        libsql::params![plan_id.to_string(), destination.to_string(), direction.to_string()],
    )
    .await
    .map_err(|e| format!("airport_transfer_candidates DELETE failed: {e}"))?;
    for (i, cand) in candidates.iter().enumerate() {
        conn.execute(
            "INSERT INTO airport_transfer_candidates \
                (plan_id, destination, direction, candidate_id, title, route, \
                 duration_min, price_yen, schedule, booking_url, notes, sort_order, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, NULL, NULL, ?10, ?11)",
            libsql::params![
                plan_id.to_string(),
                destination.to_string(),
                direction.to_string(),
                cand.id.clone(),
                cand.title.clone(),
                cand.route.clone(),
                cand.duration_min,
                cand.price_yen,
                cand.schedule.clone(),
                i as i64,
                now_db.clone()
            ],
        )
        .await
        .map_err(|e| format!("airport_transfer_candidates INSERT failed: {e}"))?;
    }

    // 3. plan_events + plan_event_data.
    let dest_process_so = next_dest_process_sort_order(
        conn,
        plan_id,
        destination,
        "process_3_transportation",
    )
    .await?;
    let timeline_base = next_timeline_sort_order(conn, plan_id).await?;

    let kv: Vec<(&str, String)> = vec![
        ("direction", direction.to_string()),
        ("status", status.to_string()),
        ("selected_id", selected.id.clone()),
        (
            "candidates_count",
            candidates.len().to_string(),
        ),
    ];

    insert_event(
        conn,
        plan_id,
        "dest_process",
        destination,
        "process_3_transportation",
        dest_process_so,
        "airport_transfer_updated",
        &now_iso,
    )
    .await?;
    insert_kv(
        conn,
        plan_id,
        "dest_process",
        destination,
        "process_3_transportation",
        dest_process_so,
        &kv,
    )
    .await?;

    let timeline_so = timeline_base;
    insert_event(
        conn,
        plan_id,
        "timeline",
        "",
        "process_3_transportation",
        timeline_so,
        "airport_transfer_updated",
        &now_iso,
    )
    .await?;
    insert_kv(
        conn,
        plan_id,
        "timeline",
        "",
        "process_3_transportation",
        timeline_so,
        &kv,
    )
    .await?;

    // 4. operation_runs + plans.version.
    let run_id = new_run_id();
    let summary = format!("{destination} {direction}");
    conn.execute(
        "INSERT INTO operation_runs \
            (run_id, plan_id, command_type, command_summary, status, \
             version_before, version_after, started_at, completed_at) \
         VALUES (?1, ?2, 'set-airport-transfer', ?3, 'completed', ?4, ?5, ?6, ?6)",
        libsql::params![
            run_id,
            plan_id.to_string(),
            summary,
            version_before,
            version_after,
            now_db.clone()
        ],
    )
    .await
    .map_err(|e| format!("operation_runs INSERT failed: {e}"))?;

    conn.execute(
        "UPDATE plans SET version = ?1, updated_at = ?2 WHERE plan_id = ?3",
        libsql::params![version_after, now_db, plan_id.to_string()],
    )
    .await
    .map_err(|e| format!("plans UPDATE failed: {e}"))?;

    Ok(version_after)
}

async fn read_version(conn: &Connection, plan_id: &str) -> Result<i64, String> {
    let mut rows = conn
        .query(
            "SELECT version FROM plans WHERE plan_id = ?1",
            libsql::params![plan_id.to_string()],
        )
        .await
        .map_err(|e| format!("plans query failed: {e}"))?;
    if let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("plans row read failed: {e}"))?
    {
        let v: i64 = row
            .get(0)
            .map_err(|e| format!("version col read failed: {e}"))?;
        return Ok(v);
    }
    Err(format!("plans row missing for plan_id={plan_id}"))
}

async fn next_dest_process_sort_order(
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
            libsql::params![plan_id.to_string(), dest.to_string(), process_id.to_string()],
        )
        .await
        .map_err(|e| format!("plan_events MAX query failed: {e}"))?;
    if let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("plan_events MAX row read failed: {e}"))?
    {
        let m: i64 = row
            .get(0)
            .map_err(|e| format!("plan_events MAX col read failed: {e}"))?;
        return Ok(m + 1);
    }
    Ok(0)
}

async fn next_timeline_sort_order(conn: &Connection, plan_id: &str) -> Result<i64, String> {
    let mut rows = conn
        .query(
            "SELECT COALESCE(MAX(sort_order), -1) AS m FROM plan_events \
             WHERE plan_id = ?1 AND scope = 'timeline'",
            libsql::params![plan_id.to_string()],
        )
        .await
        .map_err(|e| format!("plan_events MAX(timeline) query failed: {e}"))?;
    if let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("plan_events MAX(timeline) row read failed: {e}"))?
    {
        let m: i64 = row
            .get(0)
            .map_err(|e| format!("plan_events MAX(timeline) col read failed: {e}"))?;
        return Ok(m + 1);
    }
    Ok(0)
}

#[allow(clippy::too_many_arguments)]
async fn insert_event(
    conn: &Connection,
    plan_id: &str,
    scope: &str,
    destination: &str,
    process_id: &str,
    sort_order: i64,
    event: &str,
    event_at: &str,
) -> Result<(), String> {
    conn.execute(
        "DELETE FROM plan_events \
         WHERE plan_id = ?1 AND scope = ?2 AND destination = ?3 \
           AND process_id = ?4 AND sort_order = ?5",
        libsql::params![plan_id.to_string(), scope.to_string(), destination.to_string(), process_id.to_string(), sort_order],
    )
    .await
    .map_err(|e| format!("plan_events DELETE failed: {e}"))?;
    conn.execute(
        "INSERT INTO plan_events \
            (plan_id, scope, destination, process_id, sort_order, \
             event, event_at, from_state, to_state) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, NULL)",
        libsql::params![plan_id.to_string(), scope.to_string(), destination.to_string(), process_id.to_string(), sort_order, event.to_string(), event_at.to_string()],
    )
    .await
    .map_err(|e| format!("plan_events INSERT failed: {e}"))?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn insert_kv(
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
        libsql::params![plan_id.to_string(), scope.to_string(), destination.to_string(), process_id.to_string(), sort_order],
    )
    .await
    .map_err(|e| format!("plan_event_data DELETE failed: {e}"))?;
    for (k, v) in kv {
        conn.execute(
            "INSERT INTO plan_event_data \
                (plan_id, scope, destination, process_id, sort_order, key, value) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            libsql::params![plan_id.to_string(), scope.to_string(), destination.to_string(), process_id.to_string(), sort_order, k.to_string(), v.clone()],
        )
        .await
        .map_err(|e| format!("plan_event_data INSERT failed: {e}"))?;
    }
    Ok(())
}

fn new_run_id() -> String {
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
    format!(
        "{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}",
        p1, p2, p3, p4, p5
    )
}

fn now_rfc3339() -> String {
    let (year, month, day, hour, min, sec, ms) = now_civil();
    format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{min:02}:{sec:02}.{ms:03}Z"
    )
}

fn now_db_datetime() -> String {
    let (year, month, day, hour, min, sec, _) = now_civil();
    format!(
        "{year:04}-{month:02}-{day:02} {hour:02}:{min:02}:{sec:02}"
    )
}

fn now_civil() -> (i32, u32, u32, u32, u32, u32, u32) {
    use std::time::{SystemTime, UNIX_EPOCH};
    let d = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = d.as_secs() as i64;
    let ms = d.subsec_millis();
    let days = secs.div_euclid(86_400);
    let sod = secs.rem_euclid(86_400) as u32;
    let hour = sod / 3600;
    let min = (sod % 3600) / 60;
    let sec = sod % 60;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d_ = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m as u32, d_ as u32, hour, min, sec, ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_basic() {
        assert_eq!(slugify("Limousine Bus"), "limousine_bus");
        assert_eq!(slugify("NRT T1 → Shiodome"), "nrt_t1_shiodome");
        assert_eq!(slugify("---foo---bar---"), "foo_bar");
    }

    #[test]
    fn slugify_truncates_at_48() {
        let long = "a".repeat(100);
        assert_eq!(slugify(&long).len(), 48);
    }

    #[test]
    fn hash_string_djb2_base36() {
        // Sanity: "成田空港" hashes to some 6-char base36 string
        let h = hash_string("成田空港 → 日暮里 → 浜松町");
        assert!(h.len() <= 6);
        assert!(h.chars().all(|c| c.is_ascii_alphanumeric()));
    }

    #[test]
    fn parse_transfer_spec_basic() {
        let t = parse_transfer_spec(
            "arrival",
            "Limousine Bus|NRT T1 → Shiodome|85|3200|19:40 → ~21:05",
        );
        assert_eq!(t.title, "Limousine Bus");
        assert_eq!(t.route, "NRT T1 → Shiodome");
        assert_eq!(t.duration_min, Some(85));
        assert_eq!(t.price_yen, Some(3200));
        assert_eq!(t.schedule.as_deref(), Some("19:40 → ~21:05"));
        assert!(t.id.starts_with("arrival_limousine_bus_"));
    }
}
