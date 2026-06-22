// `travel snapshot-maps [--dest <slug>]` — (re)capture the dashboard's static
// route-map PNGs for a plan and upload them to R2.
//
// This is a thin, discoverable wrapper over `scripts/snapshot-maps.sh` (the
// chromeport→Leaflet→R2 pipeline). The script itself stays the source of truth
// for the rendering/upload mechanics; this command just resolves the plan's
// destination slug, locates the script, and runs it with live stdio so the
// chromeport/wrangler progress streams through.
//
// It is NOT a domain mutation (it rebuilds an external build artifact — the R2
// PNGs — and the script already calls `travel mark-maps-snapshotted` at the end
// to stamp the freshness timestamp). So it writes no audit triad here.
//
// Environment requirements (the script fails loud if unmet): real Chrome at the
// chromeport CDP endpoint, and wrangler authenticated to Cloudflare. Same
// requirements as a manual `scripts/snapshot-maps.sh` run — this command does
// not change them, it only makes the entry point discoverable via `travel`.

use std::path::PathBuf;
use std::process::Command;

pub async fn run(args: &[String], plan_id: String) -> Result<(), String> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print_usage();
        return Ok(());
    }

    // Optional explicit --dest override; else resolve the plan's active_destination.
    let dest_override = parse_dest(args)?;

    let conn = crate::db::connect_read()
        .await
        .map_err(|e| format!("failed to connect to Turso (read tier): {e}"))?;

    // Fail loud if the plan does not exist — never snapshot a phantom plan.
    if !plan_exists(&conn, &plan_id).await? {
        eprintln!("Error: plans row missing for plan_id={plan_id}");
        std::process::exit(1);
    }

    let dest = crate::cascade::common::resolve_active_destination(
        &conn,
        &plan_id,
        dest_override.as_deref(),
    )
    .await?;

    let script = locate_script()?;
    eprintln!("snapshot-maps: plan={plan_id} dest={dest}");
    eprintln!("  running {} {plan_id} {dest}", script.display());

    // Inherit stdio so chromeport/wrangler output streams live to the user.
    let status = Command::new("bash")
        .arg(&script)
        .arg(&plan_id)
        .arg(&dest)
        .status()
        .map_err(|e| format!("failed to run {}: {e}", script.display()))?;

    if !status.success() {
        eprintln!(
            "Error: snapshot-maps.sh exited with {} — see output above.",
            status.code().map(|c| c.to_string()).unwrap_or_else(|| "signal".into())
        );
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}

/// Parse an optional `--dest <slug>` override; ignore the plan-resolution flags
/// the dispatcher owns.
fn parse_dest(args: &[String]) -> Result<Option<String>, String> {
    let mut dest = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--dest" => {
                dest = args.get(i + 1).cloned();
                if dest.is_none() {
                    return Err("--dest requires a value".to_string());
                }
                i += 2;
            }
            "--plan-id" | "--travel-date" | "--travel-start" | "--travel-end" => {
                i += 2; // dispatcher-owned flag + value
            }
            other if other.starts_with("--") => {
                return Err(format!("unknown argument: {other}"));
            }
            _ => i += 1,
        }
    }
    Ok(dest)
}

/// Find `scripts/snapshot-maps.sh` relative to the repo root. The binary may be
/// invoked from anywhere, so walk up from the current dir looking for the script.
fn locate_script() -> Result<PathBuf, String> {
    let rel = "scripts/snapshot-maps.sh";
    let mut dir = std::env::current_dir()
        .map_err(|e| format!("cannot read current dir: {e}"))?;
    loop {
        let cand = dir.join(rel);
        if cand.is_file() {
            return Ok(cand);
        }
        if !dir.pop() {
            return Err(format!(
                "could not find {rel} (run from inside the travel-2026 repo)"
            ));
        }
    }
}

async fn plan_exists(conn: &libsql::Connection, plan_id: &str) -> Result<bool, String> {
    let mut rows = conn
        .query(
            "SELECT 1 FROM plans WHERE plan_id = ?1",
            libsql::params![plan_id.to_string()],
        )
        .await
        .map_err(|e| format!("plans existence query failed: {e}"))?;
    Ok(rows
        .next()
        .await
        .map_err(|e| format!("plans existence row read failed: {e}"))?
        .is_some())
}

fn print_usage() {
    println!(
        "Usage:\n  \
         travel snapshot-maps [--dest <slug>]\n  \
         (plan resolved from $TRAVEL_PLAN_ID / --plan-id; dest defaults to the\n  \
          plan's active_destination)\n\n  \
         Re-captures the dashboard route-map PNGs (plan + per-day, numbered\n  \
         markers + route polyline) via scripts/snapshot-maps.sh and uploads them\n  \
         to R2. Requires Chrome at the chromeport CDP endpoint + wrangler auth.\n  \
         Stamps the freshness timestamp on success (see check-maps-fresh)."
    );
}
