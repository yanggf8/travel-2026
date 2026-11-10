// `travel flow-decision <stage> <decision> [--mode <m>] [--reason <r>] [--source <s>]`
// — audited stage entry/skip/mode recorder (F6).
//
// Pure recorder: writes plan_events + plan_event_data + operation_runs and
// bumps plans.version. Mirrors the plan-scoped audit triad in set_activity.rs
// (timeline scope), NOT catalog_audit.rs.

use crate::cascade::common::{
    begin_immediate, finish_tx, insert_event, insert_kv_rows, next_timeline_sort_order,
    now_db_datetime, now_rfc3339, read_version, record_operation,
};
use crate::cascade::status_machine::{self, Hop, Ladder, Mode, P3, P34};

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

    // Domestic defer: when the caller records `shop mode --mode defer` for a
    // domestic destination, also move the dest-scoped package/transport
    // processes (P3_4, P3) to `skipped` so `status --full` immediately reflects
    // "shop deferred". P4 accommodation is NOT touched — a domestic trip still
    // books a stay (set-accommodation). A P3_4/P3 already booked/confirmed is
    // left alone (defer means "no shopping", not "forget a booking"). Planned in
    // a preflight before any write; any error propagates and nothing is written.
    let defer = parsed.stage == "shop"
        && parsed.decision == "mode"
        && parsed.mode.as_deref() == Some("defer");
    let mut skips: Vec<(String, &'static str, Option<String>, Vec<Hop>)> = Vec::new();
    let mut kept: Vec<String> = Vec::new();
    if defer {
        let dest = crate::cascade::common::resolve_active_destination(&conn, &plan_id, None).await?;
        if is_domestic_destination(&conn, &dest).await {
            for pid in [P34, P3] {
                let cur = travel_db::repo::process_statuses::get(&conn, &plan_id, &dest, pid).await?;
                if let Some(s @ ("booked" | "confirmed")) = cur.as_deref() {
                    kept.push(format!("{pid} stays {s}"));
                    continue;
                }
                // A missing row plans as pending; it is then inserted directly
                // (one status_changed with from_state NULL).
                let planned = status_machine::plan_path(
                    Ladder::Booking,
                    cur.as_deref().unwrap_or("pending"),
                    "skipped",
                    Mode::Any,
                )
                .map_err(|e| format!("{dest}.{pid}: {e}; nothing was written"))?;
                let hops = if cur.is_some() { planned } else { Vec::new() };
                skips.push((dest.clone(), pid, cur, hops));
            }
        }
    }

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

    begin_immediate(&conn, "flow-decision").await?;
    let written: Result<(), String> = async {
        let version_before = read_version(&conn, &plan_id).await?;
        let version_after = version_before + 1;
        let sort_order = next_timeline_sort_order(&conn, &plan_id).await?;

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

        insert_kv_rows(&conn, &plan_id, "timeline", "", "", sort_order, &kv).await?;

        for (dest, pid, cur, hops) in &skips {
            status_machine::apply_path(
                &conn,
                &plan_id,
                dest,
                pid,
                cur.as_deref(),
                hops,
                "skipped",
                &now_iso,
                &now_db,
            )
            .await?;
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
        Ok(())
    }
    .await;
    finish_tx(&conn, "flow-decision", written).await?;

    println!("✅ flow-decision recorded: {summary}");
    if !skips.is_empty() {
        println!("   domestic shop deferred → P3_4/P3 marked skipped; P4 accommodation untouched (book the stay with set-accommodation; re-run with --mode shop to re-enter selection)");
    }
    for k in &kept {
        println!("   domestic shop deferred → {k} (already booked; not skipped)");
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