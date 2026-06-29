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

    println!(
        "observation_id\tsource_id\tproduct_type\tjob_id\tattempt_id\tobservation_type\tseverity\thttp_status\tfield_name\texpected_value\tobserved_value\tdetail\tobserved_at"
    );
    for r in &rows {
        print_cell(&r.observation_id);
        print!("\t");
        print_cell(&r.source_id);
        print!("\t");
        print_opt(&r.product_type);
        print!("\t");
        print_opt(&r.job_id);
        print!("\t");
        print_opt(&r.attempt_id);
        print!("\t");
        print_cell(&r.observation_type);
        print!("\t");
        print_cell(&r.severity);
        print!("\t");
        print_opt_i64(r.http_status);
        print!("\t");
        print_opt(&r.field_name);
        print!("\t");
        print_opt(&r.expected_value);
        print!("\t");
        print_opt(&r.observed_value);
        print!("\t");
        print_opt(&r.detail);
        print!("\t");
        println!("{}", r.observed_at);
    }
    println!("observations\t{}", rows.len());
    Ok(())
}

fn print_cell(s: &str) {
    if s.is_empty() {
        print!("-");
    } else {
        print!("{s}");
    }
}

fn print_opt(s: &Option<String>) {
    match s {
        Some(v) if !v.is_empty() => print!("{v}"),
        _ => print!("-"),
    }
}

fn print_opt_i64(v: Option<i64>) {
    match v {
        Some(n) => print!("{n}"),
        None => print!("-"),
    }
}