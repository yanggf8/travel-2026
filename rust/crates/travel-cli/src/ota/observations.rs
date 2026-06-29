use crate::db;
use travel_db::repo::observations::{self, ObservationFilter};

pub async fn run(args: &[String]) -> Result<(), String> {
    let mut source_id: Option<String> = None;
    let mut observation_type: Option<String> = None;
    let mut severity: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--source" => {
                source_id = Some(args.get(i + 1).ok_or("missing --source")?.clone());
                i += 2;
            }
            "--type" => {
                observation_type = Some(args.get(i + 1).ok_or("missing --type")?.clone());
                i += 2;
            }
            "--severity" => {
                severity = Some(args.get(i + 1).ok_or("missing --severity")?.clone());
                i += 2;
            }
            _ => i += 1,
        }
    }
    if let Some(ref sev) = severity {
        if !matches!(sev.as_str(), "info" | "warn" | "error") {
            return Err(format!("Error: invalid --severity '{sev}'; use info|warn|error"));
        }
    }

    let conn = db::connect_read().await?;
    let rows = observations::list(
        &conn,
        &ObservationFilter {
            source_id: source_id.as_deref(),
            observation_type: observation_type.as_deref(),
            severity: severity.as_deref(),
        },
    )
    .await?;

    if rows.is_empty() {
        println!("observations\t0");
        return Ok(());
    }

    let header = [
        "observation_id",
        "source_id",
        "product_type",
        "job_id",
        "attempt_id",
        "observation_type",
        "block_reason_code",
        "severity",
        "http_status",
        "field_name",
        "selector",
        "expected_value",
        "observed_value",
        "duration_ms",
        "freshness_reference_at",
        "detail",
        "observed_at",
    ];
    println!("{}", header.join("\t"));
    for r in &rows {
        let cells = [
            cell(&r.observation_id),
            cell(&r.source_id),
            opt(&r.product_type),
            opt(&r.job_id),
            opt(&r.attempt_id),
            cell(&r.observation_type),
            opt(&r.block_reason_code),
            cell(&r.severity),
            opt_i64(r.http_status),
            opt(&r.field_name),
            opt(&r.selector),
            opt(&r.expected_value),
            opt(&r.observed_value),
            opt_i64(r.duration_ms),
            opt(&r.freshness_reference_at),
            opt(&r.detail),
            cell(&r.observed_at),
        ];
        println!("{}", cells.join("\t"));
    }
    println!("observations\t{}", rows.len());
    Ok(())
}

fn cell(s: &str) -> String {
    if s.is_empty() {
        "-".to_string()
    } else {
        s.to_string()
    }
}

fn opt(s: &Option<String>) -> String {
    match s {
        Some(v) if !v.is_empty() => v.clone(),
        _ => "-".to_string(),
    }
}

fn opt_i64(v: Option<i64>) -> String {
    match v {
        Some(n) => n.to_string(),
        None => "-".to_string(),
    }
}