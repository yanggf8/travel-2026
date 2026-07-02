// `travel set-process-status <process_id> <target_status> [--dest <slug>]` —
// explicit process-status ladder advancement via the shared transition graph.
//
// Mirrors the select-offer status_changed dual-bucket event pattern and uses
// `travel_db::repo::process_statuses::upsert` for domain writes. The audit
// triad stays in `cascade::common`.

use crate::cascade::common::{
    emit_status_changed, now_db_datetime, now_rfc3339, read_version, record_operation,
    resolve_active_destination, validate_transition,
};
use libsql::Connection;
use std::collections::{HashSet, VecDeque};

pub async fn run(args: &[String], plan_id: String) -> Result<(), String> {
    let parsed = parse_args(args)?;

    let conn = match crate::db::connect_write().await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error: failed to connect to Turso (write tier): {e}");
            std::process::exit(1);
        }
    };

    let destination = match resolve_active_destination(&conn, &plan_id, parsed.dest.as_deref()).await
    {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    };

    match execute(
        &conn,
        &plan_id,
        &destination,
        parsed.process_id,
        parsed.target_status,
    )
    .await
    {
        Ok(result) => {
            print_result(&plan_id, &destination, &result);
            Ok(())
        }
        Err(e) => {
            eprintln!("Error: set-process-status failed: {e}");
            std::process::exit(1);
        }
    }
}

#[derive(Debug)]
struct ParsedArgs {
    process_id: &'static str,
    target_status: &'static str,
    dest: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StatusHop {
    from: &'static str,
    to: &'static str,
}

#[derive(Debug)]
struct StatusChangeResult {
    process_id: &'static str,
    current_status: String,
    target_status: &'static str,
    hops: Vec<StatusHop>,
    version_before: Option<i64>,
    version_after: Option<i64>,
}

fn parse_args(args: &[String]) -> Result<ParsedArgs, String> {
    let mut dest: Option<String> = None;
    let mut positional: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        match a.as_str() {
            "--dest" => {
                dest = Some(arg_value(args, i, "--dest")?);
                i += 2;
            }
            "--plan-id" => {
                let _ = arg_value(args, i, "--plan-id")?;
                i += 2;
            }
            other if other.starts_with("--") => {
                return Err(format!("unknown argument: {other}"));
            }
            _ => {
                positional.push(a.clone());
                i += 1;
            }
        }
    }
    if positional.len() < 2 {
        return Err(usage_error());
    }
    let process_id = normalize_process_id(&positional[0])?;
    let target_status = normalize_status(&positional[1])?;
    Ok(ParsedArgs {
        process_id,
        target_status,
        dest,
    })
}

fn usage_error() -> String {
    "Usage: set-process-status <process_id> <target_status> [--dest <slug>] [--plan-id <id>]
Example: set-process-status process_3_transportation booked --dest okinawa_2026
Aliases: p1|date, p2|destination, p34|packages, p3|transport|flight, p4|hotel, p5|itinerary"
        .to_string()
}

fn arg_value(args: &[String], i: usize, flag: &str) -> Result<String, String> {
    args.get(i + 1)
        .cloned()
        .ok_or_else(|| format!("missing value for {flag}"))
}

fn normalize_process_id(raw: &str) -> Result<&'static str, String> {
    match raw {
        "process_1_date_anchor" | "p1" | "date" | "date-anchor" => Ok("process_1_date_anchor"),
        "process_2_destination" | "p2" | "destination" => Ok("process_2_destination"),
        "process_3_4_packages" | "p34" | "p3_4" | "packages" => Ok("process_3_4_packages"),
        "process_3_transportation"
        | "p3"
        | "transport"
        | "transportation"
        | "flight"
        | "flights" => Ok("process_3_transportation"),
        "process_4_accommodation" | "p4" | "hotel" | "hotels" | "accommodation" => {
            Ok("process_4_accommodation")
        }
        "process_5_daily_itinerary" | "p5" | "itinerary" | "daily-itinerary" => {
            Ok("process_5_daily_itinerary")
        }
        other => Err(format!("unknown process_id: {other}")),
    }
}

fn normalize_status(raw: &str) -> Result<&'static str, String> {
    match raw {
        "pending" => Ok("pending"),
        "researching" => Ok("researching"),
        "researched" => Ok("researched"),
        "selecting" => Ok("selecting"),
        "selected" => Ok("selected"),
        "populated" => Ok("populated"),
        "booking" => Ok("booking"),
        "booked" => Ok("booked"),
        "confirmed" => Ok("confirmed"),
        "skipped" => Ok("skipped"),
        other => Err(format!("unknown status: {other}")),
    }
}

fn status_literal(s: &str) -> Option<&'static str> {
    normalize_status(s).ok()
}

async fn read_process_status(
    conn: &Connection,
    plan_id: &str,
    dest: &str,
    process_id: &str,
) -> Result<Option<String>, String> {
    let mut rows = conn
        .query(
            "SELECT status FROM process_statuses \
             WHERE plan_id = ?1 AND destination = ?2 AND process_id = ?3",
            libsql::params![plan_id.to_string(), dest.to_string(), process_id.to_string()],
        )
        .await
        .map_err(|e| e.to_string())?;
    if let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
        let s: String = row.get(0).unwrap_or_default();
        return Ok(if s.is_empty() { None } else { Some(s) });
    }
    Ok(None)
}

fn legal_status_path(current: &str, target: &str) -> Result<Vec<StatusHop>, String> {
    if current == target {
        return Ok(Vec::new());
    }
    let start = status_literal(current)
        .filter(|s| !crate::cascade::common::allowed_transition_targets(s).is_empty())
        .ok_or_else(|| format!("unknown current status: {current}"))?;
    let end = status_literal(target).ok_or_else(|| format!("unknown target status: {target}"))?;

    let mut queue: VecDeque<(&'static str, Vec<StatusHop>)> = VecDeque::new();
    let mut visited: HashSet<&'static str> = HashSet::new();
    queue.push_back((start, Vec::new()));
    visited.insert(start);

    while let Some((node, path)) = queue.pop_front() {
        for &next in crate::cascade::common::allowed_transition_targets(node) {
            if visited.contains(next) {
                continue;
            }
            let mut new_path = path.clone();
            new_path.push(StatusHop { from: node, to: next });
            if next == end {
                return Ok(new_path);
            }
            visited.insert(next);
            queue.push_back((next, new_path));
        }
    }

    Err(format!("no legal status path from {current} to {target}"))
}

async fn execute(
    conn: &Connection,
    plan_id: &str,
    dest: &str,
    process_id: &'static str,
    target_status: &'static str,
) -> Result<StatusChangeResult, String> {
    let now_iso = now_rfc3339();
    let now_db = now_db_datetime();

    let current = read_process_status(conn, plan_id, dest, process_id)
        .await?
        .ok_or_else(|| {
            format!("no process_statuses row for {dest}.{process_id}")
        })?;

    let hops = legal_status_path(&current, target_status)?;

    if hops.is_empty() {
        return Ok(StatusChangeResult {
            process_id,
            current_status: current,
            target_status,
            hops,
            version_before: None,
            version_after: None,
        });
    }

    let version_before = read_version(conn, plan_id).await?;
    let version_after = version_before + 1;

    for hop in &hops {
        validate_transition(Some(hop.from), hop.to, dest, process_id)?;
        travel_db::repo::process_statuses::upsert(
            conn, plan_id, dest, process_id, hop.to, &now_db,
        )
        .await?;
        emit_status_changed(
            conn,
            plan_id,
            dest,
            process_id,
            Some(hop.from),
            hop.to,
            &now_iso,
        )
        .await?;
    }

    record_operation(
        conn,
        plan_id,
        "set-process-status",
        &format!("{dest} {process_id} {current}->{target_status}"),
        version_before,
        version_after,
        &now_db,
    )
    .await?;

    Ok(StatusChangeResult {
        process_id,
        current_status: current,
        target_status,
        hops,
        version_before: Some(version_before),
        version_after: Some(version_after),
    })
}

fn format_path(current: &str, hops: &[StatusHop]) -> String {
    let mut parts: Vec<&str> = Vec::with_capacity(hops.len() + 1);
    parts.push(current);
    for hop in hops {
        parts.push(hop.to);
    }
    parts.join(" -> ")
}

fn print_result(plan_id: &str, dest: &str, result: &StatusChangeResult) {
    if result.hops.is_empty() {
        println!(
            "No change: {}.{} already at {}",
            dest, result.process_id, result.target_status
        );
        return;
    }
    println!("Plan: {plan_id}");
    println!("Destination: {dest}");
    println!("Process: {}", result.process_id);
    println!(
        "Path: {}",
        format_path(&result.current_status, &result.hops)
    );
    if let (Some(before), Some(after)) = (result.version_before, result.version_after) {
        println!("Version: {before} -> {after}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_pending_to_booked_walks_shortest_chain() {
        assert_eq!(
            legal_status_path("pending", "booked").unwrap(),
            vec![
                StatusHop {
                    from: "pending",
                    to: "populated"
                },
                StatusHop {
                    from: "populated",
                    to: "booking"
                },
                StatusHop {
                    from: "booking",
                    to: "booked"
                },
            ]
        );
    }

    #[test]
    fn path_idempotent_is_empty() {
        assert!(legal_status_path("booked", "booked").unwrap().is_empty());
    }

    #[test]
    fn path_booking_to_confirmed() {
        assert_eq!(
            legal_status_path("booking", "confirmed").unwrap(),
            vec![
                StatusHop {
                    from: "booking",
                    to: "booked"
                },
                StatusHop {
                    from: "booked",
                    to: "confirmed"
                },
            ]
        );
    }

    #[test]
    fn path_unknown_current_fails() {
        assert!(legal_status_path("archived", "booked").is_err());
    }
}