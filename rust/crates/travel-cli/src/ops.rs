//! run-status / run-list — port of src/cli/commands/ops.ts.
//! Read-only views over the operation_runs audit table, filtered by plan_id.
//!
//! run-status [run-id]   → getOperationRun(plan_id, run_id?) detail card.
//!                          With a run-id: that row (scoped to plan). Without:
//!                          the most recent run for the plan.
//! run-list [--status s] [--limit N]
//!                       → listOperationRuns(plan_id, {status, limit}) table.
//!                         Default limit 20 (clamped 1..=100), newest first.
//!
//! Mirrors the TS column layout, icons (✅ completed / ❌ failed / ⏳ other),
//! the `before→after` version rendering, and the 19-char timestamp slice.

use crate::db;

// ── run-status ───────────────────────────────────────────────────────

pub async fn run_status(args: &[String], plan_id: String) -> Result<(), String> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("Usage:\n  travel run-status [run-id]");
        return Ok(());
    }
    // TS: const runId = ctx.args.cleanArgs[1]; — first positional (non-flag) arg.
    let run_id: Option<String> = args.iter().find(|a| !a.starts_with("--")).cloned();

    let conn = db::connect_read().await?;

    // getOperationRun(planId, runId?)
    let (sql, qparams): (&str, Vec<libsql::Value>) = if let Some(rid) = &run_id {
        (
            "SELECT run_id, plan_id, command_type, command_summary, status, \
                    version_before, version_after, started_at, completed_at, error_message \
             FROM operation_runs WHERE run_id = ?1 AND plan_id = ?2 LIMIT 1",
            vec![rid.clone().into(), plan_id.clone().into()],
        )
    } else {
        (
            "SELECT run_id, plan_id, command_type, command_summary, status, \
                    version_before, version_after, started_at, completed_at, error_message \
             FROM operation_runs WHERE plan_id = ?1 ORDER BY started_at DESC LIMIT 1",
            vec![plan_id.clone().into()],
        )
    };

    let mut rows = conn
        .query(sql, libsql::params_from_iter(qparams))
        .await
        .map_err(|e| format!("operation_runs query failed: {e}"))?;

    let row = rows
        .next()
        .await
        .map_err(|e| format!("operation_runs row read failed: {e}"))?;

    let Some(row) = row else {
        match run_id {
            Some(rid) => println!("No operation found with run_id: {rid}"),
            None => println!("No operations found for this plan."),
        }
        return Ok(());
    };

    let r_run_id: String = row.get(0).unwrap_or_default();
    let r_plan_id: String = row.get(1).unwrap_or_default();
    let r_command: String = row.get(2).unwrap_or_default();
    let r_summary: Option<String> = row.get(3).ok().flatten();
    let r_status: String = row.get(4).unwrap_or_default();
    let r_ver_before: i64 = row.get(5).unwrap_or(0);
    let r_ver_after: Option<i64> = row.get(6).ok().flatten();
    let r_started: Option<String> = row.get(7).ok().flatten();
    let r_completed: Option<String> = row.get(8).ok().flatten();
    let r_error: Option<String> = row.get(9).ok().flatten();

    let status_icon = match r_status.as_str() {
        "completed" => "✅",
        "failed" => "❌",
        _ => "⏳",
    };

    println!("\n📋 Operation Run Details");
    println!("{}", "─".repeat(50));
    println!("  Run ID:       {r_run_id}");
    println!("  Plan ID:      {r_plan_id}");
    println!("  Command:      {r_command}");
    if let Some(s) = r_summary.filter(|s| !s.is_empty()) {
        println!("  Summary:      {s}");
    }
    println!("  Status:       {status_icon} {r_status}");
    match r_ver_after {
        Some(after) => println!("  Version:      {r_ver_before} → {after}"),
        None => println!("  Version:      {r_ver_before}"),
    }
    println!("  Started:      {}", r_started.unwrap_or_default());
    if let Some(c) = r_completed.filter(|c| !c.is_empty()) {
        println!("  Completed:    {c}");
    }
    if let Some(e) = r_error.filter(|e| !e.is_empty()) {
        println!("  Error:        {e}");
    }
    println!();
    Ok(())
}

// ── run-list ─────────────────────────────────────────────────────────

pub async fn run_list(args: &[String], plan_id: String) -> Result<(), String> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("Usage:\n  travel run-list [--status completed|failed|started] [--limit N]");
        return Ok(());
    }
    let status_filter = option_value(args, "--status");
    let limit_raw = option_value(args, "--limit")
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(20);
    // listOperationRuns clamps to 1..=100.
    let limit = limit_raw.clamp(1, 100);

    let conn = db::connect_read().await?;

    let (sql, qparams): (String, Vec<libsql::Value>) = match &status_filter {
        Some(st) => (
            format!(
                "SELECT run_id, command_type, command_summary, status, \
                        version_before, version_after, started_at \
                 FROM operation_runs WHERE plan_id = ?1 AND status = ?2 \
                 ORDER BY started_at DESC LIMIT {limit}"
            ),
            vec![plan_id.clone().into(), st.clone().into()],
        ),
        None => (
            format!(
                "SELECT run_id, command_type, command_summary, status, \
                        version_before, version_after, started_at \
                 FROM operation_runs WHERE plan_id = ?1 \
                 ORDER BY started_at DESC LIMIT {limit}"
            ),
            vec![plan_id.clone().into()],
        ),
    };

    let mut rows = conn
        .query(&sql, libsql::params_from_iter(qparams))
        .await
        .map_err(|e| format!("operation_runs list query failed: {e}"))?;

    struct RunRow {
        command_type: String,
        command_summary: Option<String>,
        status: String,
        version_before: i64,
        version_after: Option<i64>,
        started_at: Option<String>,
    }

    let mut runs: Vec<RunRow> = Vec::new();
    while let Some(r) = rows
        .next()
        .await
        .map_err(|e| format!("operation_runs list row read failed: {e}"))?
    {
        runs.push(RunRow {
            command_type: r.get(1).unwrap_or_default(),
            command_summary: r.get(2).ok().flatten(),
            status: r.get(3).unwrap_or_default(),
            version_before: r.get(4).unwrap_or(0),
            version_after: r.get(5).ok().flatten(),
            started_at: r.get(6).ok().flatten(),
        });
    }

    if runs.is_empty() {
        println!("No operations found.");
        return Ok(());
    }

    println!("\n📋 Recent Operations ({} results)", runs.len());
    println!("{}", "─".repeat(90));
    println!(
        "{} │ {} │ {} │ {} │ {}",
        pad_end("Status", 5),
        pad_end("Command", 22),
        pad_end("Summary", 30),
        pad_end("Ver", 7),
        pad_end("Started", 20),
    );
    println!("{}", "─".repeat(90));

    for run in &runs {
        let icon = match run.status.as_str() {
            "completed" => "✅",
            "failed" => "❌",
            _ => "⏳",
        };
        let ver = match run.version_after {
            Some(after) => format!("{}→{}", run.version_before, after),
            None => format!("{}", run.version_before),
        };
        let started = run
            .started_at
            .as_deref()
            .map(|s| s.chars().take(19).collect::<String>())
            .unwrap_or_else(|| "-".to_string());
        let summary = run
            .command_summary
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or("-");
        let summary = summary.chars().take(30).collect::<String>();
        let command = if run.command_type.is_empty() {
            "-"
        } else {
            run.command_type.as_str()
        };

        println!(
            "{} │ {} │ {} │ {} │ {}",
            pad_end(icon, 5),
            pad_end(command, 22),
            pad_end(&summary, 30),
            pad_end(&ver, 7),
            pad_end(&started, 20),
        );
    }
    println!("{}", "─".repeat(90));
    println!();
    Ok(())
}

// ── helpers ──────────────────────────────────────────────────────────

fn option_value(args: &[String], name: &str) -> Option<String> {
    let mut i = 0;
    while i < args.len() {
        if args[i] == name {
            return args.get(i + 1).cloned();
        }
        i += 1;
    }
    None
}

/// Pad to `width` columns, matching JS `String.padEnd` for the ASCII
/// headers/values here (char-count is equivalent for ASCII).
fn pad_end(s: &str, width: usize) -> String {
    let len = s.chars().count();
    if len >= width {
        s.to_string()
    } else {
        let mut out = s.to_string();
        out.push_str(&" ".repeat(width - len));
        out
    }
}
