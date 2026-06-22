// `travel plans` — list travel plans + their date anchors from Turso.
// Read-only; plain-text table. Ports src/cli/commands/plans.ts + the listPlans /
// formatPlanList / planStatus logic from src/cli/shared/plan-resolver.ts.

use crate::db;

struct Anchor {
    destination: String,
    start_date: String,
    end_date: String,
}

struct PlanSummary {
    plan_id: String,
    active_destination: String,
    anchors: Vec<Anchor>,
    window_start: String,
    window_end: String,
}

pub async fn run() -> Result<(), String> {
    let plans = list_plans().await?;
    println!("{}", format_plan_list(&plans, &today_iso()));
    Ok(())
}

async fn list_plans() -> Result<Vec<PlanSummary>, String> {
    let conn = db::connect_read().await?;
    let mut rows = conn
        .query(
            "SELECT pm.plan_id, pm.active_destination,
                    da.destination, da.start_date, da.end_date,
                    p1.set_out_date AS window_start, p1.return_date AS window_end
             FROM plan_metadata pm
             LEFT JOIN date_anchors da ON da.plan_id = pm.plan_id
             LEFT JOIN plan_root_date_anchor p1 ON p1.plan_id = pm.plan_id
             WHERE pm.plan_id NOT IN (
                 SELECT plan_id FROM plans WHERE deleted_at IS NOT NULL
             )
             ORDER BY pm.updated_at DESC, da.start_date ASC",
            (),
        )
        .await
        .map_err(|err| format!("failed to query plans from Turso: {err}"))?;

    // group rows by plan_id, preserving first-seen order (query already ordered)
    let mut order: Vec<String> = Vec::new();
    let mut summaries: std::collections::HashMap<String, PlanSummary> = std::collections::HashMap::new();

    while let Some(row) = rows
        .next()
        .await
        .map_err(|err| format!("failed to read plan row: {err}"))?
    {
        let plan_id: String = row.get(0).unwrap_or_default();
        if plan_id.is_empty() {
            continue;
        }
        let active_destination: String = row.get(1).unwrap_or_default();
        let dest: String = row.get(2).unwrap_or_default();
        let start: String = row.get(3).unwrap_or_default();
        let end: String = row.get(4).unwrap_or_default();
        let window_start: String = row.get(5).unwrap_or_default();
        let window_end: String = row.get(6).unwrap_or_default();

        let summary = summaries.entry(plan_id.clone()).or_insert_with(|| {
            order.push(plan_id.clone());
            PlanSummary {
                plan_id: plan_id.clone(),
                active_destination,
                anchors: Vec::new(),
                window_start,
                window_end,
            }
        });
        if !dest.is_empty() || !start.is_empty() {
            summary.anchors.push(Anchor {
                destination: dest,
                start_date: start,
                end_date: end,
            });
        }
    }

    Ok(order.into_iter().filter_map(|id| summaries.remove(&id)).collect())
}

/// Lifecycle of a single date range vs `today` (ISO YYYY-MM-DD): "active" while
/// today is within [start, end], "upcoming" before it, "past" after. Shared
/// single source of truth so `plans` and `status` agree.
pub fn lifecycle(start_date: &str, end_date: &str, today: &str) -> &'static str {
    if start_date <= today && today <= end_date {
        "active"
    } else if today < start_date {
        "upcoming"
    } else {
        "past"
    }
}

/// Today as ISO YYYY-MM-DD (UTC; `TRAVEL_TODAY` overrides for tests). Public so
/// other views (e.g. `status`) compute the same "today" the plan list uses.
pub fn today_iso_pub() -> String {
    today_iso()
}

fn plan_status(plan: &PlanSummary, today: &str) -> String {
    // A multi-anchor plan is "active" if ANY anchor is active, else "upcoming"
    // if ANY anchor is still ahead, else "past". Reuse the single-range helper.
    if plan
        .anchors
        .iter()
        .any(|a| lifecycle(&a.start_date, &a.end_date, today) == "active")
    {
        return "active".to_string();
    }
    if plan
        .anchors
        .iter()
        .any(|a| lifecycle(&a.start_date, &a.end_date, today) == "upcoming")
    {
        return "upcoming".to_string();
    }
    "past".to_string()
}

fn format_plan_list(plans: &[PlanSummary], today: &str) -> String {
    if plans.is_empty() {
        return "No travel plans found.".to_string();
    }
    let mut lines = vec![
        "PLAN_ID          STATUS    ACTIVE_DEST      DATE_ANCHORS".to_string(),
        "---------------  --------  ---------------  ------------------------------".to_string(),
    ];
    for plan in plans {
        let anchors = if plan.anchors.is_empty() {
            "(no date anchor)".to_string()
        } else {
            plan.anchors
                .iter()
                .map(|a| format!("{}: {}..{}", a.destination, a.start_date, a.end_date))
                .collect::<Vec<_>>()
                .join("; ")
        };
        let window = if !plan.window_start.is_empty() || !plan.window_end.is_empty() {
            let s = if plan.window_start.is_empty() { "?" } else { &plan.window_start };
            let e = if plan.window_end.is_empty() { "?" } else { &plan.window_end };
            format!(" window={s}..{e}")
        } else {
            String::new()
        };
        let status = plan_status(plan, today);
        lines.push(format!(
            "{:<16} {:<9} {:<16} {}{}",
            plan.plan_id, status, plan.active_destination, anchors, window
        ));
    }
    lines.join("\n")
}

fn today_iso() -> String {
    // YYYY-MM-DD in local time, std-only (no chrono dep in travel-cli).
    // Use the date command's contract indirectly via SystemTime → not ideal for tz,
    // so prefer chrono if already available; travel-cli has none, so compute from
    // a simple UTC epoch breakdown is overkill — shell out is banned. Use the
    // TRAVEL_TODAY override for tests, else a minimal UTC date.
    if let Ok(v) = std::env::var("TRAVEL_TODAY") {
        if !v.trim().is_empty() {
            return v;
        }
    }
    minimal_utc_date()
}

/// Minimal UTC YYYY-MM-DD from the system clock, no external crates.
fn minimal_utc_date() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (secs / 86_400) as i64;
    // civil date from days since 1970-01-01 (Howard Hinnant's algorithm)
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_classifies_past_active_upcoming() {
        // okinawa-2026 (2026-06-12 → 06-16) vs today 2026-06-22 → past
        assert_eq!(lifecycle("2026-06-12", "2026-06-16", "2026-06-22"), "past");
        // during the trip → active (inclusive on both ends)
        assert_eq!(lifecycle("2026-06-12", "2026-06-16", "2026-06-12"), "active");
        assert_eq!(lifecycle("2026-06-12", "2026-06-16", "2026-06-14"), "active");
        assert_eq!(lifecycle("2026-06-12", "2026-06-16", "2026-06-16"), "active");
        // before the trip → upcoming
        assert_eq!(lifecycle("2026-06-12", "2026-06-16", "2026-06-01"), "upcoming");
        // day after end → past
        assert_eq!(lifecycle("2026-06-12", "2026-06-16", "2026-06-17"), "past");
    }
}
