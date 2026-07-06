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

    println!("flow-decision recorded: {summary}");
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