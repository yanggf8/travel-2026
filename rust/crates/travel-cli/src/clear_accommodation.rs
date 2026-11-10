// `travel clear-accommodation --hotel <name> [--room-type <type>] [--dest <slug>] [--plan-id <id>]`
// — domestic Taiwan accommodation cancellation (pure Rust, no JSON).
//
// Preflight before any delete: the booked rows to clear, and P4's path back to
// `selecting` (`status_machine::plan_path(.., Mode::Any)` — e.g. booked →
// cancelled → selecting, or confirmed → booked → cancelled → selecting). No P4
// row or no legal path → Err and nothing is deleted. Then one BEGIN IMMEDIATE …
// COMMIT deletes the bookings, writes the hops and the audit triad; output is
// printed only after COMMIT.

use crate::cascade::common::{
    begin_immediate, finish_tx, now_db_datetime, now_rfc3339, read_version, record_operation,
    resolve_active_destination,
};
use crate::cascade::status_machine::{self, Ladder, Mode, P4};

#[derive(Debug)]
struct Args {
    hotel: String,
    room_type: Option<String>,
    dest: Option<String>,
}

pub async fn run(raw: &[String], plan_id: String) -> Result<(), String> {
    let args = parse_args(raw)?;

    let conn = crate::db::connect_write().await.map_err(|e| format!("failed to connect to Turso (write tier): {e}"))?;

    let dest = match resolve_active_destination(&conn, &plan_id, args.dest.as_deref()).await {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    };

    // Find bookings to clear (category=accommodation, status=booked, hotel match)
    let like_pattern = format!("%{}%", args.hotel);
    let mut sql = "SELECT booking_key, title FROM bookings_current WHERE trip_id=?1 AND destination=?2 AND category='accommodation' AND status='booked' AND title LIKE ?3".to_string();
    let mut params: Vec<String> = vec![plan_id.clone(), dest.clone(), like_pattern];
    if let Some(rt) = &args.room_type {
        sql.push_str(" AND title LIKE ?4");
        params.push(format!("%{}%", rt));
    }

    let mut rows = match params.len() {
        3 => conn.query(&sql, libsql::params![params[0].clone(), params[1].clone(), params[2].clone()]).await.map_err(|e| e.to_string())?,
        4 => conn.query(&sql, libsql::params![params[0].clone(), params[1].clone(), params[2].clone(), params[3].clone()]).await.map_err(|e| e.to_string())?,
        _ => unreachable!(),
    };

    let mut keys: Vec<(String, String)> = Vec::new();
    while let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
        let k: String = row.get(0).unwrap_or_default();
        let t: String = row.get(1).unwrap_or_default();
        if !k.is_empty() {
            keys.push((k, t));
        }
    }

    if keys.is_empty() {
        eprintln!("No booked accommodation matching hotel '{}' room '{:?}' for dest '{dest}' — nothing to clear", args.hotel, args.room_type);
        std::process::exit(1);
    }

    // Preflight: P4's path back to selecting, before any delete.
    let target = "selecting";
    let current = travel_db::repo::process_statuses::get(&conn, &plan_id, &dest, P4)
        .await?
        .ok_or_else(|| format!("no process_statuses row for {dest}.{P4}; nothing was cleared"))?;
    let hops = status_machine::plan_path(Ladder::Booking, &current, target, Mode::Any)
        .map_err(|e| format!("{dest}.{P4}: {e}; nothing was cleared"))?;

    let now_iso = now_rfc3339();
    let now_db = now_db_datetime();

    begin_immediate(&conn, "clear-accommodation").await?;
    let written: Result<(), String> = async {
        let version_before = read_version(&conn, &plan_id).await?;
        let version_after = version_before + 1;

        // Delete from bookings_current and legacy bookings
        for (k, _) in &keys {
            conn.execute("DELETE FROM bookings_current WHERE booking_key=?1", libsql::params![k.clone()]).await.map_err(|e| e.to_string())?;
        }
        // Legacy bookings table uses composite PK (destination, offer_id) — clear by destination + hotel_name pattern.
        conn.execute("DELETE FROM bookings WHERE destination=?1 AND hotel_name LIKE ?2", libsql::params![dest.clone(), format!("%{}%", args.hotel)])
            .await
            .map_err(|e| format!("legacy bookings delete failed: {e}"))?;

        status_machine::apply_path(
            &conn,
            &plan_id,
            &dest,
            P4,
            Some(&current),
            &hops,
            target,
            &now_iso,
            &now_db,
        )
        .await?;

        record_operation(&conn, &plan_id, "clear-accommodation", &format!("clear {} ({})", args.hotel, keys.len()), version_before, version_after, &now_db).await?;
        Ok(())
    }
    .await;
    finish_tx(&conn, "clear-accommodation", written).await?;

    for (_, title) in &keys {
        println!("✅ Cleared accommodation: {} for {} plan {}", title, dest, plan_id);
    }
    let path = if hops.is_empty() {
        format!("already {target}")
    } else {
        status_machine::format_path(&current, &hops, " → ")
    };
    println!("   P4 accommodation -> selecting ({path}; {} booking(s) removed)", keys.len());
    Ok(())
}

fn parse_args(raw: &[String]) -> Result<Args, String> {
    let mut hotel: Option<String> = None;
    let mut room_type: Option<String> = None;
    let mut dest: Option<String> = None;
    let mut i = 0;
    while i < raw.len() {
        let k = raw[i].as_str();
        match k {
            "--hotel" => {
                let v = raw.get(i + 1).cloned().ok_or_else(|| "--hotel requires a value".to_string())?;
                if v.trim().is_empty() {
                    return Err("--hotel cannot be empty".to_string());
                }
                hotel = Some(v);
                i += 2;
            }
            "--room-type" => {
                let v = raw.get(i + 1).cloned().ok_or_else(|| "--room-type requires a value".to_string())?;
                room_type = Some(v);
                i += 2;
            }
            "--dest" | "--destination" => {
                let v = raw.get(i + 1).cloned().ok_or_else(|| format!("{k} requires a value"))?;
                dest = Some(v);
                i += 2;
            }
            "--help" | "-h" => {
                print_usage();
                std::process::exit(0);
            }
            other if other.starts_with("--") => {
                if other == "--plan-id" || other == "--travel-date" || other == "--travel-start" || other == "--travel-end" {
                    if raw.get(i + 1).is_some_and(|v| !v.starts_with("--")) {
                        i += 2;
                    } else {
                        i += 1;
                    }
                    continue;
                }
                return Err(format!("unknown flag for clear-accommodation: {other}"));
            }
            other => return Err(format!("unexpected positional argument: {other}")),
        }
    }
    let hotel = hotel.ok_or_else(|| "--hotel <name> is required.\nUsage: travel clear-accommodation --hotel <name> [--room-type <type>] [--dest <slug>] [--plan-id <id>]".to_string())?;
    Ok(Args { hotel, room_type, dest })
}

fn print_usage() {
    println!("Usage:\n  travel clear-accommodation --hotel <name> [--room-type <type>] [--dest <slug>] [--plan-id <id>]\n  (removes booked accommodation and reverts P4 to selecting)");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_required_hotel() {
        let o = parse_args(&a(&["--hotel", "海論"])).unwrap();
        assert_eq!(o.hotel, "海論");
        assert!(o.room_type.is_none());
    }

    #[test]
    fn parses_optional_room_type() {
        let o = parse_args(&a(&["--hotel", "海論", "--room-type", "海景雙人房"])).unwrap();
        assert_eq!(o.room_type.as_deref(), Some("海景雙人房"));
    }

    #[test]
    fn rejects_missing_hotel() {
        let e = parse_args(&a(&[])).unwrap_err();
        assert!(e.contains("--hotel"));
    }
}
