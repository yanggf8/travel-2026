use crate::db;
use crate::ota::common;
use travel_db::repo::ota_jobs;

pub async fn run_claim(args: &[String]) -> Result<(), String> {
    let mut worker = "default-worker".to_string();
    let mut lease_seconds: i64 = 120;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--worker" => {
                worker = args
                    .get(i + 1)
                    .ok_or("missing value for --worker")?
                    .clone();
                i += 2;
            }
            "--lease-seconds" => {
                lease_seconds = args
                    .get(i + 1)
                    .ok_or("missing value for --lease-seconds")?
                    .parse()
                    .map_err(|_| "--lease-seconds must be an integer".to_string())?;
                i += 2;
            }
            _ => i += 1,
        }
    }
    if lease_seconds <= 0 {
        return Err(format!(
            "Error: --lease-seconds must be > 0 (got {lease_seconds}); a non-positive lease would \
             expire before now and be reaped immediately"
        ));
    }

    let conn = db::connect_write().await?;
    let now = common::now_iso();
    let lease_expires = common::lease_expires(&now, lease_seconds)?;
    let claimed = ota_jobs::claim(&conn, &worker, &now, &lease_expires).await?;
    match claimed {
        Some(c) => {
            println!("job_id\t{}", c.job_id);
            println!("claim_token\t{}", c.claim_token);
            println!("source_id\t{}", c.source_id);
            println!("product_type\t{}", c.product_type);
            println!("status\tclaimed");
            println!("lease_expires_at\t{lease_expires}");
        }
        None => {
            println!("status\tno_job");
        }
    }
    Ok(())
}

pub async fn run_heartbeat(args: &[String]) -> Result<(), String> {
    let positional = common::positionals(args, &["--lease-seconds"]);
    if positional.len() < 2 {
        return Err(
            "Usage: travel ota heartbeat <job_id> <claim_token> [--lease-seconds N]".to_string(),
        );
    }
    let job_id = positional[0].as_str();
    let claim_token = positional[1].as_str();
    let mut lease_seconds: i64 = 120;
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--lease-seconds" {
            lease_seconds = args
                .get(i + 1)
                .ok_or("missing value for --lease-seconds")?
                .parse()
                .map_err(|_| "--lease-seconds must be an integer".to_string())?;
            i += 2;
        } else {
            i += 1;
        }
    }
    if lease_seconds <= 0 {
        return Err(format!(
            "Error: --lease-seconds must be > 0 (got {lease_seconds})"
        ));
    }

    let conn = db::connect_write().await?;
    let now = common::now_iso();
    let lease_expires = common::lease_expires(&now, lease_seconds)?;
    let affected = ota_jobs::heartbeat(&conn, job_id, claim_token, &now, &lease_expires).await?;
    if affected == 0 {
        return Err(format!(
            "Error: heartbeat rejected — job_id={job_id} token mismatch or job not claimed/running"
        ));
    }
    println!("job_id\t{job_id}");
    println!("heartbeat_at\t{now}");
    println!("lease_expires_at\t{lease_expires}");
    Ok(())
}

pub async fn run_finish(args: &[String]) -> Result<(), String> {
    let positional = common::positionals(args, &["--status", "--blocked"]);
    if positional.len() < 2 {
        return Err(
            "Usage: travel ota finish <job_id> <claim_token> \
             --status succeeded|failed|blocked [--blocked <code>]"
                .to_string(),
        );
    }
    let job_id = positional[0].as_str();
    let claim_token = positional[1].as_str();
    let mut status: Option<String> = None;
    let mut blocked_code: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--status" => {
                status = Some(
                    args.get(i + 1)
                        .ok_or("missing value for --status")?
                        .clone(),
                );
                i += 2;
            }
            "--blocked" => {
                blocked_code = Some(
                    args.get(i + 1)
                        .ok_or("missing value for --blocked")?
                        .clone(),
                );
                i += 2;
            }
            _ => i += 1,
        }
    }
    let status = status.ok_or("Error: --status is required (succeeded|failed|blocked)")?;
    if !matches!(status.as_str(), "succeeded" | "failed" | "blocked") {
        return Err(format!("Error: invalid --status '{status}'"));
    }
    if status == "blocked" {
        let code = blocked_code.as_deref().ok_or(
            "Error: --blocked <code> required when --status blocked",
        )?;
        let conn = db::connect_write().await?;
        if !common::table_exists(&conn, "coverage_block_reasons", "code", code).await? {
            return Err(format!(
                "Error: blocked_reason_code '{code}' not in coverage_block_reasons"
            ));
        }
    }

    let conn = db::connect_write().await?;
    let now = common::now_iso();
    let affected = ota_jobs::finish(
        &conn,
        job_id,
        claim_token,
        &status,
        blocked_code.as_deref(),
        &now,
    )
    .await?;
    if affected == 0 {
        return Err(format!(
            "Error: finish rejected — job_id={job_id} token mismatch or job not claimed/running"
        ));
    }
    println!("job_id\t{job_id}");
    println!("status\t{status}");
    if let Some(code) = blocked_code {
        println!("blocked_reason_code\t{code}");
    }
    Ok(())
}

pub async fn run_reap_stale(args: &[String]) -> Result<(), String> {
    let mut now = common::now_iso();
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--now" {
            now = args
                .get(i + 1)
                .ok_or("missing value for --now")?
                .clone();
            i += 2;
        } else {
            i += 1;
        }
    }
    // reap_stale compares --now lexically against stored ISO leases; a non-canonical form
    // would mis-order. Reject anything but %Y-%m-%dT%H:%M:%SZ.
    common::validate_iso_z(&now)?;

    let conn = db::connect_write().await?;
    let reaped = ota_jobs::reap_stale(&conn, &now).await?;
    // `reaped` = total leases acted on (requeued + parked-as-failed); the breakdown follows.
    println!("reaped\t{}", reaped.requeued + reaped.failed);
    println!("requeued\t{}", reaped.requeued);
    println!("failed\t{}", reaped.failed);
    println!("now\t{now}");
    Ok(())
}