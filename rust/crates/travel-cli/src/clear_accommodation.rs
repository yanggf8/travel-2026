// `travel clear-accommodation --hotel <name> [--room-type <type>] [--dest <slug>] [--plan-id <id>]`
// — domestic Taiwan accommodation cancellation (pure Rust, no JSON).

use crate::cascade::common::{
    emit_status_changed, now_db_datetime, now_rfc3339, read_version, record_operation,
    resolve_active_destination, validate_transition,
};

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

    let now_iso = now_rfc3339();
    let now_db = now_db_datetime();
    let version_before = read_version(&conn, &plan_id).await?;
    let version_after = version_before + 1;

    // Delete from bookings_current and legacy bookings
    for (k, _) in &keys {
        conn.execute("DELETE FROM bookings_current WHERE booking_key=?1", libsql::params![k.clone()]).await.map_err(|e| e.to_string())?;
    }
    // Legacy bookings table uses composite PK (destination, offer_id) — clear by destination + hotel_name pattern if possible
    // For domestic, we stored offer_id as synthetic; best effort delete via hotel_name
    let _ = conn.execute("DELETE FROM bookings WHERE destination=?1 AND hotel_name LIKE ?2", libsql::params![dest.clone(), format!("%{}%", args.hotel)]).await;

    // Advance process_statuses P4: booked -> cancelled -> selecting (legal path via common.rs)
    let current = read_process_status(&conn, &plan_id, &dest, "process_4_accommodation").await?;
    if let Some(cur) = current {
        // Try booked -> cancelled -> selecting, fallback to any legal path to selecting
        let target = "selecting";
        if let Ok(hops) = legal_status_path(&cur, target) {
            for hop in &hops {
                validate_transition(Some(&hop.from), &hop.to, &dest, "process_4_accommodation")?;
                travel_db::repo::process_statuses::upsert(&conn, &plan_id, &dest, "process_4_accommodation", &hop.to, &now_db).await?;
                emit_status_changed(&conn, &plan_id, &dest, "process_4_accommodation", Some(&hop.from), &hop.to, &now_iso).await?;
            }
        }
    }

    record_operation(&conn, &plan_id, "clear-accommodation", &format!("clear {} ({})", args.hotel, keys.len()), version_before, version_after, &now_db).await?;

    for (_, title) in &keys {
        println!("✅ Cleared accommodation: {} for {} plan {}", title, dest, plan_id);
    }
    println!("   P4 accommodation -> selecting ({} booking(s) removed)", keys.len());
    Ok(())
}

async fn read_process_status(conn: &libsql::Connection, plan_id: &str, dest: &str, process_id: &str) -> Result<Option<String>, String> {
    let mut rows = conn.query("SELECT status FROM process_statuses WHERE plan_id=?1 AND destination=?2 AND process_id=?3", libsql::params![plan_id.to_string(), dest.to_string(), process_id.to_string()]).await.map_err(|e| e.to_string())?;
    if let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
        let s: String = row.get(0).unwrap_or_default();
        return Ok(if s.is_empty() { None } else { Some(s) });
    }
    Ok(None)
}

fn legal_status_path(current: &str, target: &str) -> Result<Vec<StatusHop>, String> {
    use std::collections::{HashSet, VecDeque};
    if current == target {
        return Ok(Vec::new());
    }
    let start = match current {
        "pending" | "researching" | "researched" | "selecting" | "selected" | "populated" | "booking" | "booked" | "cancelled" | "confirmed" | "skipped" => current.to_string(),
        _ => return Err(format!("unknown current status: {current}")),
    };
    let end = target;
    let mut queue: VecDeque<(String, Vec<StatusHop>)> = VecDeque::new();
    let mut visited: HashSet<String> = HashSet::new();
    queue.push_back((start.clone(), Vec::new()));
    visited.insert(start);
    while let Some((node, path)) = queue.pop_front() {
        for &next in crate::cascade::common::allowed_transition_targets(&node) {
            if visited.contains(next) {
                continue;
            }
            let mut new_path = path.clone();
            new_path.push(StatusHop { from: node.clone(), to: next.to_string() });
            if next == end {
                return Ok(new_path);
            }
            visited.insert(next.to_string());
            queue.push_back((next.to_string(), new_path));
        }
    }
    Err(format!("no legal status path from {current} to {target}"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StatusHop {
    from: String,
    to: String,
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
