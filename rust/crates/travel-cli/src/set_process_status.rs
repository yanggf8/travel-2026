// `travel set-process-status <process_id> <target_status> [--dest <slug>]` —
// explicit process-status move along the process's own ladder
// (cascade::status_machine: Anchor for P1/P2, Booking for P3_4/P3/P4,
// Itinerary for P5).
//
// Preflight first (no writes): the target must be on the process's ladder, the
// row must exist, `plan_path(.., Mode::Any)` must find a legal path, and a P5
// move to `confirmed` must pass the publish gate
// (`validate::collect_publish_issues` — any Block refuses the confirm). Then one
// BEGIN IMMEDIATE … COMMIT writes every hop (`apply_path`: upsert +
// status_changed per hop) and the audit triad back half; output is printed only
// after COMMIT.

use crate::cascade::common::{
    begin_immediate, finish_tx, now_db_datetime, now_rfc3339, read_version, record_operation,
    resolve_active_destination,
};
use crate::cascade::status_machine::{self, Hop, Mode};
use libsql::Connection;

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

#[derive(Debug)]
struct StatusChangeResult {
    process_id: &'static str,
    current_status: String,
    target_status: &'static str,
    hops: Vec<Hop>,
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
            f if crate::plan_resolver::is_resolver_flag(f) => {
                let _ = arg_value(args, i, f)?;
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
Aliases: p1|date, p2|destination, p34|packages, p3|transport|flight, p4|hotel, p5|itinerary
Ladders: p1/p2 anchor = pending|researching|researched|confirmed|skipped;
         p34/p3/p4 booking = pending|researching|researched|selecting|selected|populated|booking|booked|confirmed|skipped|cancelled
                             (confirmed only via booked);
         p5 itinerary = pending|researching|researched|confirmed|skipped
         (p5 confirmed is refused while `validate publish` reports a blocker)"
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
        "cancelled" => Ok("cancelled"),
        other => Err(format!("unknown status: {other}")),
    }
}

async fn execute(
    conn: &Connection,
    plan_id: &str,
    dest: &str,
    process_id: &'static str,
    target_status: &'static str,
) -> Result<StatusChangeResult, String> {
    // ── preflight: nothing below writes until every check passed ──────────
    let ladder = status_machine::ladder(process_id)?;
    if !status_machine::on_ladder(ladder, target_status) {
        return Err(format!(
            "status {target_status} is not on the {} ladder",
            ladder.name()
        ));
    }

    let current = travel_db::repo::process_statuses::get(conn, plan_id, dest, process_id)
        .await?
        .ok_or_else(|| format!("no process_statuses row for {dest}.{process_id}"))?;

    let hops = status_machine::plan_path(ladder, &current, target_status, Mode::Any)?;

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

    if process_id == status_machine::P5 && target_status == "confirmed" {
        p5_confirm_gate(conn, plan_id, dest).await?;
    }

    // ── one transaction: hops + audit ─────────────────────────────────────
    let now_iso = now_rfc3339();
    let now_db = now_db_datetime();
    begin_immediate(conn, "set-process-status").await?;
    let written: Result<(i64, i64), String> = async {
        let version_before = read_version(conn, plan_id).await?;
        let version_after = version_before + 1;
        status_machine::apply_path(
            conn,
            plan_id,
            dest,
            process_id,
            Some(&current),
            &hops,
            target_status,
            &now_iso,
            &now_db,
        )
        .await?;
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
        Ok((version_before, version_after))
    }
    .await;
    let (version_before, version_after) = finish_tx(conn, "set-process-status", written).await?;

    Ok(StatusChangeResult {
        process_id,
        current_status: current,
        target_status,
        hops,
        version_before: Some(version_before),
        version_after: Some(version_after),
    })
}

/// P5 → confirmed means the itinerary is final: refuse while the publish gate
/// reports any blocker (warnings and info do not block).
async fn p5_confirm_gate(conn: &Connection, plan_id: &str, dest: &str) -> Result<(), String> {
    let issues = crate::validate::collect_publish_issues(conn, plan_id, dest).await?;
    let blockers: Vec<String> = issues
        .iter()
        .filter(|i| i.is_block())
        .map(|i| format!("  ❌ [{}] {}", i.category, i.message))
        .collect();
    if blockers.is_empty() {
        return Ok(());
    }
    Err(format!(
        "P5 confirm refused for {dest}: {} publish blocker(s); nothing was written. Fix them, then re-run (full report: travel validate publish --plan-id {plan_id} --dest {dest})\n{}",
        blockers.len(),
        blockers.join("\n")
    ))
}

fn print_result(plan_id: &str, dest: &str, result: &StatusChangeResult) {
    if result.hops.is_empty() {
        println!(
            "✅ No change: {}.{} already at {}",
            dest, result.process_id, result.target_status
        );
        return;
    }
    println!("\n🔀 Setting process status:");
    println!("   Plan: {plan_id}");
    println!("   Destination: {dest}");
    println!("   Process: {}", result.process_id);
    println!(
        "   Path: {}",
        status_machine::format_path(&result.current_status, &result.hops, " -> ")
    );
    if let (Some(before), Some(after)) = (result.version_before, result.version_after) {
        println!("   Version: {before} -> {after}");
    }
    println!(
        "✅ {} → {}",
        result.process_id, result.target_status
    );
}
