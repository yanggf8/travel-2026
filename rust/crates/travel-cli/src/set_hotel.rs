// `travel set-hotel ...` — port of src/cli/commands/set-hotel.ts. NO CASCADE.
//
// Mirrors the TS path:
//   1. UPSERT hotels.name / check_in / notes (only fields passed) — upsert
//      (not plain UPDATE) so a booking-first plan with no offer-cascade
//      skeleton row still persists.
//   2. DELETE-then-reinsert hotel_access_lines (the access list is
//      pipe-delimited from --access; each non-empty segment becomes a
//      sort_order=N row)
//   3. INSERT 1 dest_process + 1 timeline plan_event (hotel_updated,
//      process_4_accommodation) with plan_event_data KV rows mirroring
//      the input fields (name / access / checkIn / notes — exact key
//      names match the TS `HotelInput` field names because
//      `data: { ...input }` is the spread)
//   4. INSERT operation_runs (audit) + UPDATE plans.version
//
// Verified no-cascade: cascade_dirty_flags UNCHANGED.

use libsql::Connection;
use travel_db::repo::hotels;

#[derive(Default, Debug)]
struct HotelInput {
    name: Option<String>,
    access: Vec<String>,    // split from --access by '|'
    check_in: Option<String>,
    notes: Option<String>,
}

pub async fn run(
    args: &[String],
    plan_id: String,
) -> Result<(), String> {
    let input = parse_args(args)?;

    if input.name.is_none()
        && input.access.is_empty()
        && input.check_in.is_none()
        && input.notes.is_none()
    {
        eprintln!("Error: set-hotel requires at least one of --name, --access, --check-in, --note");
        eprintln!("Example: set-hotel --dest kyoto_2026 --name \"APA Hotel Kyoto Ekimae\" --check-in 2026-02-24 --access \"JR Kyoto Station 3min\"");
        std::process::exit(1);
    }

    let conn = match crate::db::connect_write().await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error: failed to connect to Turso (write tier): {e}");
            std::process::exit(1);
        }
    };

    let destination = match read_destination(&conn, &plan_id, args).await {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    };

    // Print the user-facing header (mirrors TS).
    println!("\n🏨 Setting hotel:");
    println!("   Destination: {destination}");
    if let Some(n) = &input.name {
        println!("   Name: {n}");
    }
    if let Some(c) = &input.check_in {
        println!("   Check-in: {c}");
    }
    if !input.access.is_empty() {
        println!("   Access: {}", input.access.join(" | "));
    }
    if let Some(n) = &input.notes {
        println!("   Notes: {n}");
    }

    match execute(&conn, &plan_id, &destination, &input).await {
        Ok(_) => {
            println!("✅ Hotel updated");
            Ok(())
        }
        Err(e) => {
            eprintln!("Error: set-hotel failed: {e}");
            std::process::exit(1);
        }
    }
}

fn parse_args(args: &[String]) -> Result<HotelInput, String> {
    let mut input = HotelInput::default();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        match a.as_str() {
            "--name" => {
                input.name = Some(arg_value(args, i, "--name")?);
                i += 2;
            }
            "--access" => {
                let v = arg_value(args, i, "--access")?;
                input.access = v
                    .split('|')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                i += 2;
            }
            "--check-in" => {
                input.check_in = Some(arg_value(args, i, "--check-in")?);
                i += 2;
            }
            "--note" => {
                input.notes = Some(arg_value(args, i, "--note")?);
                i += 2;
            }
            // `--dest <slug>` is advertised in the Example and consumed
            // separately by read_destination(); accept-and-skip it here so
            // the catch-all below doesn't reject the documented invocation.
            "--dest" => {
                let _ = arg_value(args, i, "--dest")?;
                i += 2;
            }
            f if crate::plan_resolver::is_resolver_flag(f) => {
                // any plan-selection flag (--plan-id / --plan-path / --travel-date /
                // --travel-start / --travel-end) is consumed by the resolver; skip flag + value
                i += 2;
            }
            other if other.starts_with("--") => {
                return Err(format!("unknown argument: {other}"));
            }
            _ => i += 1,
        }
    }
    Ok(input)
}

fn arg_value(args: &[String], i: usize, flag: &str) -> Result<String, String> {
    args.get(i + 1)
        .cloned()
        .ok_or_else(|| format!("missing value for {flag}"))
}

async fn read_destination(
    conn: &Connection,
    plan_id: &str,
    args: &[String],
) -> Result<String, String> {
    // Find --dest <slug> in args, else read plan_metadata.active_destination.
    let mut dest_override = None;
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--dest" {
            dest_override = args.get(i + 1).cloned();
            break;
        }
        i += 1;
    }
    crate::cascade::common::resolve_active_destination(conn, plan_id, dest_override.as_deref())
        .await
}

async fn execute(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    input: &HotelInput,
) -> Result<i64, String> {
    let now_iso = now_rfc3339();
    let now_db = now_db_datetime();

    let version_before = read_version(conn, plan_id).await?;
    let version_after = version_before + 1;

    // 1 + 2. UPSERT hotels (only the provided columns, keyed on the
    //    (plan_id, destination) PK) + replace hotel_access_lines — domain writes
    //    via the DAL. The upsert handles the booking-first case (a hand-scaffolded
    //    plan with no skeleton hotels row) by INSERT…ON CONFLICT, and always runs
    //    so the access lines are never orphaned.
    hotels::upsert(
        conn,
        plan_id,
        destination,
        &hotels::HotelWrite {
            name: input.name.clone(),
            check_in: input.check_in.clone(),
            notes: input.notes.clone(),
            access: input.access.clone(),
        },
        &now_db,
    )
    .await?;

    // 3. plan_events + plan_event_data.
    let dest_process_so = next_dest_process_sort_order(
        conn,
        plan_id,
        destination,
        "process_4_accommodation",
    )
    .await?;
    let timeline_base = next_timeline_sort_order(conn, plan_id).await?;

    // The TS spread `data: { ...input }` includes all HotelInput keys
    // (name, access, checkIn, notes) — keys with undefined are still
    // emitted with `undefined`, which `eventDataToKv` flattens to a
    // single '_value' entry, but in our `object` branch of
    // eventDataToKv, undefined values become the string "undefined".
    //
    // From the captured TS data, the emitted keys are exactly:
    //   name, access, checkIn, notes  (in the same order as HotelInput)
    // Missing fields are OMITTED from the data spread (the TS object's
    // `...input` doesn't have undefined keys when those were not
    // provided). So we emit ONLY the keys the user actually provided.
    let mut kv: Vec<(&str, String)> = Vec::new();
    if let Some(n) = &input.name {
        kv.push(("name", n.clone()));
    }
    if !input.access.is_empty() {
        kv.push(("access", input.access.join(", ")));
    }
    if let Some(c) = &input.check_in {
        kv.push(("checkIn", c.clone()));
    }
    if let Some(no) = &input.notes {
        kv.push(("notes", no.clone()));
    }

    insert_event(
        conn,
        plan_id,
        "dest_process",
        destination,
        "process_4_accommodation",
        dest_process_so,
        "hotel_updated",
        &now_iso,
    )
    .await?;
    insert_kv(
        conn,
        plan_id,
        "dest_process",
        destination,
        "process_4_accommodation",
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
        "process_4_accommodation",
        timeline_so,
        "hotel_updated",
        &now_iso,
    )
    .await?;
    insert_kv(
        conn,
        plan_id,
        "timeline",
        "",
        "process_4_accommodation",
        timeline_so,
        &kv,
    )
    .await?;

    // 4. operation_runs audit row + plans.version bump (shared audit-triad back half).
    let summary = format!(
        "{} {}",
        destination,
        input.name.as_deref().unwrap_or("")
    );
    crate::cascade::common::record_operation(
        conn,
        plan_id,
        "set-hotel",
        &summary,
        version_before,
        version_after,
        &now_db,
    )
    .await?;

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
    fn parse_args_minimal() {
        let i = parse_args(&["--name".to_string(), "Hilton".to_string()]).unwrap();
        assert_eq!(i.name.as_deref(), Some("Hilton"));
        assert!(i.access.is_empty());
    }

    #[test]
    fn parse_args_access_pipe_split() {
        let i = parse_args(&[
            "--access".to_string(),
            "JR Tokyo 5min | Metro Ginza 3min".to_string(),
        ])
        .unwrap();
        assert_eq!(i.access, vec!["JR Tokyo 5min", "Metro Ginza 3min"]);
    }

    #[test]
    fn parse_args_all_fields() {
        let i = parse_args(&[
            "--name".to_string(),
            "Park Hyatt".to_string(),
            "--check-in".to_string(),
            "2026-02-13".to_string(),
            "--note".to_string(),
            "King bed".to_string(),
            "--access".to_string(),
            "Shinjuku Stn 7min".to_string(),
        ])
        .unwrap();
        assert_eq!(i.name.as_deref(), Some("Park Hyatt"));
        assert_eq!(i.check_in.as_deref(), Some("2026-02-13"));
        assert_eq!(i.notes.as_deref(), Some("King bed"));
        assert_eq!(i.access, vec!["Shinjuku Stn 7min"]);
    }
}
