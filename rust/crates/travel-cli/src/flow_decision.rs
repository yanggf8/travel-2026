// `travel flow-decision <stage> <decision> [--mode <m>] [--reason <r>] [--source <s>]`
// — audited stage entry/skip/mode recorder (F6).
//
// Pure recorder: writes plan_events + plan_event_data + operation_runs and
// bumps plans.version. Mirrors the plan-scoped audit triad in set_activity.rs
// (timeline scope), NOT catalog_audit.rs.

use crate::cascade::common::{
    insert_event, insert_kv_rows, next_timeline_sort_order, now_db_datetime, now_rfc3339,
    read_version, record_operation,
};

pub const STAGES: &[&str] = &["shaping", "itinerary", "shop", "publish"];
pub const DECISIONS: &[&str] = &["enter", "skip", "mode"];
pub const MODES: &[&str] = &["shop", "ingest-known", "defer"];

pub async fn run(args: &[String]) -> Result<(), String> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print_usage();
        return Ok(());
    }

    let parsed = parse_and_validate(args)?;

    let plan_id = crate::plan_resolver::resolve_plan_id(args).await?;
    let conn = crate::db::connect_write()
        .await
        .map_err(|e| format!("failed to connect to Turso (write tier): {e}"))?;

    let version_before = read_version(&conn, &plan_id).await?;
    let version_after = version_before + 1;
    let sort_order = next_timeline_sort_order(&conn, &plan_id).await?;
    let now_iso = now_rfc3339();
    let now_db = now_db_datetime();

    let mut kv: Vec<(&str, String)> = vec![
        ("stage", parsed.stage.clone()),
        ("decision", parsed.decision.clone()),
    ];
    if let Some(m) = &parsed.mode {
        kv.push(("mode", m.clone()));
    }
    if let Some(r) = &parsed.reason {
        kv.push(("reason", r.clone()));
    }
    if let Some(s) = &parsed.source {
        kv.push(("source", s.clone()));
    }

    let mut summary = format!("{} {}", parsed.stage, parsed.decision);
    if let Some(m) = &parsed.mode {
        summary.push_str(&format!(" mode={m}"));
    }
    if let Some(r) = &parsed.reason {
        summary.push_str(&format!(" reason={r}"));
    }

    insert_event(
        &conn,
        &plan_id,
        "timeline",
        "",
        "",
        sort_order,
        "flow_decision",
        &now_iso,
        None,
        None,
    )
    .await?;

    insert_kv_rows(
        &conn,
        &plan_id,
        "timeline",
        "",
        "",
        sort_order,
        &kv,
    )
    .await?;

    // Domestic defer: when the caller records `shop mode --mode defer` for a
    // domestic destination, also flip the dest-scoped package/transport process
    // so `status --full` immediately reflects "shop deferred" instead of staying
    // stuck on `pending` until a manual `set-process-status` is run. This is the
    // domestic analogue of international's select-offer → cascade populate.
    // Best-effort and idempotent: only act on `shop` + `defer` + domestic dest;
    // any failure here must not undo the flow_decision event already written above.
    if parsed.stage == "shop"
        && parsed.decision == "mode"
        && parsed.mode.as_deref() == Some("defer")
    {
        if let Ok(dest) = crate::cascade::common::resolve_active_destination(
            &conn, &plan_id, None,
        )
        .await
        {
            if is_domestic_destination(&conn, &dest).await {
                let defer_targets = ["process_3_4_packages", "process_3_transportation"];
                for pid in defer_targets {
                    let _ = advance_process_to_skipped(&conn, &plan_id, &dest, pid).await;
                }
                // For domestic lodging: only defer if P4 is still pending/selecting
                // (already booked → don't clobber the stay).
                let p4 = read_process_status(&conn, &plan_id, &dest, "process_4_accommodation")
                    .await
                    .unwrap_or(None);
                if matches!(p4.as_deref(), None | Some("pending") | Some("selecting") | Some("researching") | Some("researched")) {
                    let _ = advance_process_to_skipped(
                        &conn, &plan_id, &dest, "process_4_accommodation",
                    )
                    .await;
                }
            }
        }
    }

    record_operation(
        &conn,
        &plan_id,
        "flow-decision",
        &summary,
        version_before,
        version_after,
        &now_db,
    )
    .await?;

    println!("✅ flow-decision recorded: {summary}");
    if parsed.stage == "shop"
        && parsed.decision == "mode"
        && parsed.mode.as_deref() == Some("defer")
    {
        println!("   domestic shop deferred → P3/P4 marked skipped (re-run with --mode shop to re-enter selection)");
    }
    Ok(())
}

#[derive(Debug)]
struct Parsed {
    stage: String,
    decision: String,
    mode: Option<String>,
    reason: Option<String>,
    source: Option<String>,
}

fn parse_and_validate(args: &[String]) -> Result<Parsed, String> {
    if args.is_empty() {
        return Err(usage_error());
    }

    let mut mode: Option<String> = None;
    let mut reason: Option<String> = None;
    let mut source: Option<String> = None;
    let mut positional: Vec<String> = Vec::new();
    let mut i = 0;

    while i < args.len() {
        match args[i].as_str() {
            "--mode" => {
                mode = Some(
                    args.get(i + 1)
                        .ok_or_else(|| "missing value for --mode".to_string())?
                        .clone(),
                );
                i += 2;
            }
            "--reason" => {
                reason = Some(
                    args.get(i + 1)
                        .ok_or_else(|| "missing value for --reason".to_string())?
                        .clone(),
                );
                i += 2;
            }
            "--source" => {
                source = Some(
                    args.get(i + 1)
                        .ok_or_else(|| "missing value for --source".to_string())?
                        .clone(),
                );
                i += 2;
            }
            f if crate::plan_resolver::is_resolver_flag(f) => {
                if args.get(i + 1).is_none() {
                    return Err(format!("missing value for {f}"));
                }
                i += 2;
            }
            other if other.starts_with("--") => {
                return Err(format!("unknown argument: {other}"));
            }
            other => {
                positional.push(other.to_string());
                i += 1;
            }
        }
    }

    if positional.len() < 2 {
        return Err(usage_error());
    }
    if positional.len() > 2 {
        return Err(format!(
            "unexpected positional argument: {}",
            positional[2]
        ));
    }

    let stage = positional[0].clone();
    let decision = positional[1].clone();

    if !STAGES.contains(&stage.as_str()) {
        return Err(format!(
            "invalid stage '{stage}': must be one of {}",
            STAGES.join("|")
        ));
    }
    if !DECISIONS.contains(&decision.as_str()) {
        return Err(format!(
            "invalid decision '{decision}': must be one of {}",
            DECISIONS.join("|")
        ));
    }

    if decision == "mode" {
        let m = mode
            .as_deref()
            .ok_or_else(|| "--mode is required when decision=mode".to_string())?;
        if !MODES.contains(&m) {
            return Err(format!(
                "invalid mode '{m}': must be one of {}",
                MODES.join("|")
            ));
        }
    } else if mode.is_some() {
        return Err(format!(
            "--mode is only allowed when decision=mode (got decision={decision})"
        ));
    }

    let reason = reason
        .map(|r| r.trim().to_string())
        .filter(|r| !r.is_empty());
    let source = source
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    Ok(Parsed {
        stage,
        decision,
        mode,
        reason,
        source,
    })
}

fn usage_error() -> String {
    "Usage: travel flow-decision <stage> <decision> [--mode <m>] [--reason <r>] [--source <s>] [--plan-id <id>]\n\
     stage: shaping|itinerary|shop|publish\n\
     decision: enter|skip|mode\n\
     --mode: shop|ingest-known|defer (required when decision=mode)"
        .to_string()
}

fn print_usage() {
    println!(
        "Usage:\n  travel flow-decision <stage> <decision> [--mode <m>] [--reason <r>] [--source <s>] [--plan-id <id>]\n\n\
         Record an audited flow-routing decision on the plan timeline.\n\
         stage: shaping | itinerary | shop | publish\n\
         decision: enter | skip | mode\n\
         --mode: shop | ingest-known | defer (required when decision=mode)"
    );
}

async fn is_domestic_destination(conn: &libsql::Connection, dest: &str) -> bool {
    // Cheap domestic check: destination_config.currency != JPY (jiufen = TWD, tokyo = JPY).
    // Falls back to slug heuristic if DB row missing.
    if let Ok(mut rows) = conn
        .query(
            "SELECT currency FROM destination_config WHERE slug = ?1 LIMIT 1",
            libsql::params![dest.to_string()],
        )
        .await
    {
        if let Ok(Some(row)) = rows.next().await {
            let cur: String = row.get(0).unwrap_or_default();
            if !cur.is_empty() {
                return cur.to_uppercase() != "JPY";
            }
        }
    }
    // Fallback: non-japan slugs (jiufen, etc.) are domestic; known JP slugs contain _2026 with JP currency pattern.
    // If unknown, treat as domestic only if slug doesn't look like a Japan trio.
    !matches!(dest, "tokyo_2026" | "kyoto_2026" | "osaka_2026" | "osaka_kyoto_2026" | "nagoya_2026" | "okinawa_2026")
}

async fn read_process_status(
    conn: &libsql::Connection,
    plan_id: &str,
    dest: &str,
    process_id: &str,
) -> Result<Option<String>, String> {
    let mut rows = conn
        .query(
            "SELECT status FROM process_statuses WHERE plan_id = ?1 AND destination = ?2 AND process_id = ?3",
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

async fn advance_process_to_skipped(
    conn: &libsql::Connection,
    plan_id: &str,
    dest: &str,
    process_id: &str,
) -> Result<(), String> {
    let cur = read_process_status(conn, plan_id, dest, process_id)
        .await?
        .unwrap_or_else(|| "pending".to_string());
    if cur == "skipped" {
        return Ok(());
    }
    // Use shared transition graph to find shortest path to skipped.
    let hops = {
        use std::collections::{HashSet, VecDeque};
        let mut queue: VecDeque<(&str, Vec<(&str, &str)>)> = VecDeque::new();
        let mut visited: HashSet<&str> = HashSet::new();
        // Seed with current — need static str for visited; leak the owned cur for queue search by matching against known literals.
        let start: &str = match cur.as_str() {
            "pending" | "researching" | "researched" | "selecting" | "selected" | "populated" | "booking" | "booked" | "confirmed" | "skipped" => cur.as_str(),
            _ => "pending",
        };
        queue.push_back((start, Vec::new()));
        visited.insert(start);
        let mut found: Option<Vec<(&str, &str)>> = None;
        while let Some((node, path)) = queue.pop_front() {
            for &next in crate::cascade::common::allowed_transition_targets(node) {
                if visited.contains(next) {
                    continue;
                }
                let mut np = path.clone();
                np.push((node, next));
                if next == "skipped" {
                    found = Some(np);
                    break;
                }
                visited.insert(next);
                queue.push_back((next, np));
            }
            if found.is_some() {
                break;
            }
        }
        found.ok_or_else(|| format!("no legal path from {cur} to skipped"))?
    };
    let now_iso = crate::cascade::common::now_rfc3339();
    let now_db = crate::cascade::common::now_db_datetime();
    for (from, to) in hops {
        crate::cascade::common::validate_transition(Some(from), to, dest, process_id)?;
        travel_db::repo::process_statuses::upsert(conn, plan_id, dest, process_id, to, &now_db).await?;
        crate::cascade::common::emit_status_changed(conn, plan_id, dest, process_id, Some(from), to, &now_iso).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constants_match_spec() {
        assert_eq!(
            STAGES,
            &["shaping", "itinerary", "shop", "publish"]
        );
        assert_eq!(DECISIONS, &["enter", "skip", "mode"]);
        assert_eq!(MODES, &["shop", "ingest-known", "defer"]);
    }

    #[test]
    fn mode_decision_requires_mode_flag() {
        let err = parse_and_validate(&[
            "shop".to_string(),
            "mode".to_string(),
        ])
        .unwrap_err();
        assert!(err.contains("--mode is required"));
    }

    #[test]
    fn enter_rejects_mode_flag() {
        let err = parse_and_validate(&[
            "shaping".to_string(),
            "enter".to_string(),
            "--mode".to_string(),
            "shop".to_string(),
        ])
        .unwrap_err();
        assert!(err.contains("--mode is only allowed"));
    }

    #[test]
    fn invalid_stage_rejected() {
        let err = parse_and_validate(&[
            "bogus".to_string(),
            "enter".to_string(),
        ])
        .unwrap_err();
        assert!(err.contains("invalid stage"));
    }

    #[test]
    fn empty_reason_omitted() {
        let p = parse_and_validate(&[
            "shop".to_string(),
            "enter".to_string(),
            "--reason".to_string(),
            "   ".to_string(),
        ])
        .unwrap();
        assert!(p.reason.is_none());
    }

    #[test]
    fn summary_fields_only_stage_decision_reason_mode() {
        let p = parse_and_validate(&[
            "shop".to_string(),
            "mode".to_string(),
            "--mode".to_string(),
            "ingest-known".to_string(),
            "--reason".to_string(),
            "known_flights".to_string(),
            "--source".to_string(),
            "agent".to_string(),
        ])
        .unwrap();
        assert_eq!(p.mode.as_deref(), Some("ingest-known"));
        assert_eq!(p.reason.as_deref(), Some("known_flights"));
        assert_eq!(p.source.as_deref(), Some("agent"));
    }
}