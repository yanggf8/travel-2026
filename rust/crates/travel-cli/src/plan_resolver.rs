// `travel resolve-plan` — debug subcommand that runs the precedence ladder
// from src/cli/shared/plan-resolver.ts (248 LOC) and prints the resolved
// plan_id + source + note. READ-ONLY port. No mutations.
//
// Mirrors all 10 branches of `resolvePlanFromSummaries()`:
//   1. explicit --plan-id
//   2. $TRAVEL_PLAN_ID env var
//   3. --plan-path (derive plan_id via toPlanId)
//   4. --travel-date (0 → throw; >1 → throw ambiguous; 1 → source "date")
//   5. --travel-start / --travel-end (same as above)
//   6. single ACTIVE plan today
//   7. single UPCOMING plan
//   8. exactly one plan exists
//   9. most-recently-updated plan
//  10. else → "No travel plans found in DB..."
//
// The resolver is intentionally a read-only debug surface. Existing
// read views (status/bookings/transport/itinerary) keep reading
// TRAVEL_PLAN_ID env directly; they are already byte-parity-proven
// against the TypeScript CLI. This module will become load-bearing
// once mutation commands land (a later Phase 4 task).
//
// We duplicate the small `PlanSummary` struct, the 3-status
// `plan_status`, the `format_plan_list` table, and `today_iso` from
// `plans.rs` rather than refactoring `plans.rs` to share types —
// the existing `travel plans` listing is byte-parity-proven and
// refactoring it would risk breaking that surface.

use crate::db;
use regex::Regex;
use std::sync::OnceLock;

/// Anchor for one destination inside a plan.
#[derive(Debug, Clone)]
pub struct PlanAnchor {
    pub destination: String,
    pub start_date: String, // ISO YYYY-MM-DD
    pub end_date: String,   // ISO YYYY-MM-DD
    /// Days of the anchor. Public API field mirroring the TS
    /// `PlanAnchor.days` — not consumed by the resolver itself.
    #[allow(dead_code)]
    pub days: i64,
}

/// Planning window for the plan (P1 root date anchor, optional).
#[derive(Debug, Clone)]
pub struct PlanningWindow {
    /// P1 status. Public API field mirroring the TS
    /// `PlanningWindow.status`.
    #[allow(dead_code)]
    pub status: String,
    pub start_date: Option<String>,
    pub end_date: Option<String>,
    /// Duration in days. Public API field mirroring the TS
    /// `PlanningWindow.durationDays`.
    #[allow(dead_code)]
    pub duration_days: Option<i64>,
}

/// Per-plan summary assembled from plan_metadata + date_anchors +
/// plan_root_date_anchor. Mirrors src/cli/shared/plan-resolver.ts
/// `PlanSummary`.
#[derive(Debug, Clone)]
pub struct PlanSummary {
    pub plan_id: String,
    pub active_destination: String,
    pub updated_at: String,
    pub anchors: Vec<PlanAnchor>,
    pub planning_window: Option<PlanningWindow>,
}

/// Inputs the precedence ladder consumes.
#[derive(Debug, Clone, Default)]
pub struct ResolveInput {
    pub explicit_plan_id: Option<String>,
    pub env_plan_id: Option<String>,
    pub plan_path: Option<String>,
    pub date: Option<String>,
    pub range_start: Option<String>,
    pub range_end: Option<String>,
    pub today: Option<String>,
}

/// Output of the ladder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedPlan {
    pub plan_id: String,
    /// One of "explicit" | "env" | "plan-path" | "date" | "active" | "upcoming" | "latest".
    pub source: &'static str,
    pub note: Option<String>,
}

const SRC_EXPLICIT: &str = "explicit";
const SRC_ENV: &str = "env";
const SRC_PLAN_PATH: &str = "plan-path";
const SRC_DATE: &str = "date";
const SRC_ACTIVE: &str = "active";
const SRC_UPCOMING: &str = "upcoming";
const SRC_LATEST: &str = "latest";

fn iso_date_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\d{4}-\d{2}-\d{2}$").expect("valid regex"))
}

/// Mirror of `isIsoDate` from plan-resolver.ts. Accepts only strict
/// YYYY-MM-DD strings.
pub fn is_iso_date(value: Option<&str>) -> bool {
    match value {
        Some(v) => iso_date_re().is_match(v),
        None => false,
    }
}

/// Validate + return a date string, or throw the same message TS does.
fn normalize_date_input(value: Option<&str>) -> Result<Option<String>, String> {
    match value {
        None => Ok(None),
        Some(v) if is_iso_date(Some(v)) => Ok(Some(v.to_string())),
        Some(v) => Err(format!("Invalid date \"{v}\". Expected YYYY-MM-DD.")),
    }
}

/// Inclusive overlap check (anchors OR planning window) — mirrors
/// `plansWithDate()` from plan-resolver.ts lines 206-212.
fn plans_with_date(plans: &[PlanSummary], start: &str, end: &str) -> Vec<PlanSummary> {
    plans
        .iter()
        .filter(|plan| {
            if plan.anchors.iter().any(|a| a.start_date.as_str() <= end && a.end_date.as_str() >= start) {
                return true;
            }
            if let Some(window) = &plan.planning_window
                && let (Some(ws), Some(we)) = (window.start_date.as_deref(), window.end_date.as_deref())
                && ws <= end && we >= start
            {
                return true;
            }
            false
        })
        .cloned()
        .collect()
}

/// Pick a single match or throw. TS: `pickSinglePlan` (lines 214-224).
fn pick_single_plan(
    matches: Vec<PlanSummary>,
    none_message: &str,
    many_message: &str,
    source: &'static str,
    all_plans: &[PlanSummary],
) -> Result<ResolvedPlan, String> {
    if matches.len() == 1 {
        return Ok(ResolvedPlan {
            plan_id: matches[0].plan_id.clone(),
            source,
            note: None,
        });
    }
    if matches.is_empty() {
        let list = format_plan_list_for_resolver(all_plans);
        return Err(format!("{none_message}\n\n{list}"));
    }
    Err(format_ambiguous_plan_error(many_message, &matches))
}

/// Format the "ambiguous" error. TS: `formatAmbiguousPlanError`
/// (line 226). The narrow-with-flags message is byte-exact.
fn format_ambiguous_plan_error(message: &str, plans: &[PlanSummary]) -> String {
    let list = format_plan_list_for_resolver(plans);
    format!("{message}\nUse --plan-id <id> or narrow with --travel-date/--travel-start/--travel-end.\n\n{list}")
}

/// 3-status version (active / upcoming / past). Matches the listing
/// output of `travel plans` (the resolver's error format reuses the
/// listing table for consistency). The TS source has a 5-status
/// version (active / upcoming / planning / candidate / past) —
///
/// the planning/candidate branches are unreachable in the resolver's
/// error context because the 5-status version downgrades to
/// "upcoming" when window.endDate >= today; that branch is a
/// strict subset of the upcoming check. Verified by inspection: the
/// current live data (2026-06-08) yields "past" for both seeded
/// plans under either function.
fn plan_status(plan: &PlanSummary, today: &str) -> String {
    if plan
        .anchors
        .iter()
        .any(|a| a.start_date.as_str() <= today && a.end_date.as_str() >= today)
    {
        return "active".to_string();
    }
    if plan.anchors.iter().any(|a| a.end_date.as_str() >= today) {
        return "upcoming".to_string();
    }
    "past".to_string()
}

/// Render the same table the `travel plans` listing prints. The
/// table has headers, a separator, and one row per plan with an
/// optional window suffix. Byte-for-byte identical to
/// `plans::format_plan_list`.
pub fn format_plan_list_for_resolver(plans: &[PlanSummary]) -> String {
    let today = today_iso();
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
        let window = match &plan.planning_window {
            Some(w) if w.start_date.is_some() || w.end_date.is_some() => {
                let s = w.start_date.as_deref().unwrap_or("?");
                let e = w.end_date.as_deref().unwrap_or("?");
                format!(" window={s}..{e}")
            }
            _ => String::new(),
        };
        let status = plan_status(plan, &today);
        lines.push(format!(
            "{:<16} {:<9} {:<16} {}{}",
            plan.plan_id, status, plan.active_destination, anchors, window
        ));
    }
    lines.join("\n")
}

/// Mirror of `toPlanId` from src/utils/plan-id.ts. The TS does
/// `slug.replace(/_/g, '-')`; we also strip a trailing file
/// extension (the `--plan-path` source is typically a `.md` path).
/// For full parity with `StateManager.derivePlanId()` see TODO below.
/// Faithful port of `StateManager.derivePlanId(planPath)` (src/state/state-manager.ts:171).
/// `data/trips/<X>/...` → `<X>`; everything else → `path:<sha1(canonical_path)[:12]>`.
/// Canonicalization mirrors TS: realpath (std::fs::canonicalize) of the resolved path,
/// falling back to the plain absolute path when realpath fails (non-existent file).
/// Backslashes normalized to '/'. NOTE: --plan-path is largely vestigial in the DB-only
/// world; this exists for parity with the TS resolver's branch #3.
fn derive_plan_id(plan_path: &str) -> String {
    use sha1::{Digest, Sha1};
    use std::path::Path;

    let normalize = |p: &str| p.replace('\\', "/");
    let canonical_abs = |p: &str| -> String {
        let abs = std::path::absolute(p)
            .unwrap_or_else(|_| Path::new(p).to_path_buf());
        let canon = std::fs::canonicalize(&abs).unwrap_or(abs);
        normalize(&canon.to_string_lossy())
    };

    let canonical_path = canonical_abs(plan_path);
    let cwd = std::env::current_dir()
        .map(|d| d.to_string_lossy().into_owned())
        .unwrap_or_default();
    let cwd_canon = canonical_abs(&cwd);

    // path.relative(cwd_canon, canonical_path), normalized.
    let rel = path_relative(&cwd_canon, &canonical_path);

    // ^data/trips/([^/]+)/
    #[allow(clippy::collapsible_if)]
    if let Some(rest) = rel.strip_prefix("data/trips/") {
        if let Some((seg, tail)) = rest.split_once('/') {
            if !seg.is_empty() && !tail.is_empty() {
                return seg.to_string();
            }
        }
    }

    let hash = Sha1::digest(canonical_path.as_bytes());
    let hex: String = hash.iter().map(|b| format!("{b:02x}")).collect();
    format!("path:{}", &hex[..12])
}

/// Minimal POSIX `path.relative(from, to)` for already-normalized absolute paths
/// (forward slashes). Matches Node's path.relative for the inputs derive_plan_id feeds it.
fn path_relative(from: &str, to: &str) -> String {
    let from_parts: Vec<&str> = from.split('/').filter(|s| !s.is_empty()).collect();
    let to_parts: Vec<&str> = to.split('/').filter(|s| !s.is_empty()).collect();
    let common = from_parts
        .iter()
        .zip(to_parts.iter())
        .take_while(|(a, b)| a == b)
        .count();
    let ups = from_parts.len() - common;
    let mut out: Vec<String> = std::iter::repeat_n("..".to_string(), ups).collect();
    out.extend(to_parts[common..].iter().map(|s| s.to_string()));
    out.join("/")
}

/// Mirror of `todayIso()` plus the `TRAVEL_TODAY` env override. The
/// resolver keeps a `today` parameter on `ResolveInput` for unit
/// tests; this is the production-side read.
pub fn today_iso() -> String {
    if let Ok(v) = std::env::var("TRAVEL_TODAY")
        && !v.trim().is_empty()
    {
        return v;
    }
    minimal_utc_date()
}

/// The plan-SELECTION flags the top-level dispatcher's `resolve_plan_id` consumes
/// to pick the plan (see `parse_args_inner`). Each takes a following VALUE.
/// SINGLE SOURCE OF TRUTH: a per-command parser that rejects unknown flags must
/// skip these (flag + value) — they belong to the resolver, not the command.
/// Keep this list in lock-step with the match arms in `parse_args_inner`.
pub const RESOLVER_VALUE_FLAGS: &[&str] = &[
    "--plan-id",
    "--plan-path",
    "--travel-date",
    "--travel-start",
    "--travel-end",
];

/// True if `flag` is one of the resolver's plan-selection flags (each consumes a
/// following value). A command parser skips `i += 2` when this returns true.
pub fn is_resolver_flag(flag: &str) -> bool {
    RESOLVER_VALUE_FLAGS.contains(&flag)
}

/// Minimal UTC YYYY-MM-DD from the system clock, no external
/// crates. Copied from `plans::minimal_utc_date` (Howard Hinnant's
/// civil-from-days algorithm).
fn minimal_utc_date() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (secs / 86_400) as i64;
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

/// The 10-branch precedence ladder. Mirrors
/// `resolvePlanFromSummaries()` (plan-resolver.ts lines 118-179)
/// EXACTLY.
pub fn resolve_plan_from_summaries(
    input: &ResolveInput,
    plans: &[PlanSummary],
) -> Result<ResolvedPlan, String> {
    // 1. explicit --plan-id
    if let Some(id) = &input.explicit_plan_id
        && !id.is_empty()
    {
        return Ok(ResolvedPlan {
            plan_id: id.clone(),
            source: SRC_EXPLICIT,
            note: None,
        });
    }
    // 2. $TRAVEL_PLAN_ID env var
    if let Some(id) = &input.env_plan_id
        && !id.is_empty()
    {
        return Ok(ResolvedPlan {
            plan_id: id.clone(),
            source: SRC_ENV,
            note: None,
        });
    }
    // 3. --plan-path (derive plan_id)
    if let Some(path) = &input.plan_path
        && !path.is_empty()
    {
        return Ok(ResolvedPlan {
            plan_id: derive_plan_id(path),
            source: SRC_PLAN_PATH,
            note: None,
        });
    }

    let today = input.today.clone().unwrap_or_else(today_iso);
    let date = normalize_date_input(input.date.as_deref())?;
    let range_start = normalize_date_input(input.range_start.as_deref())?;
    let range_end = normalize_date_input(input.range_end.as_deref())?;

    // 4. --travel-date
    if let Some(d) = date.as_deref() {
        return pick_single_plan(
            plans_with_date(plans, d, d),
            &format!("No travel plan contains {d}."),
            &format!("Multiple travel plans contain {d}."),
            SRC_DATE,
            plans,
        );
    }

    // 5. --travel-start / --travel-end
    if range_start.is_some() || range_end.is_some() {
        let start = range_start
            .clone()
            .or_else(|| range_end.clone())
            .unwrap();
        let end = range_end
            .clone()
            .or_else(|| range_start.clone())
            .unwrap();
        return pick_single_plan(
            plans_with_date(plans, &start, &end),
            &format!("No travel plan overlaps {start} to {end}."),
            &format!("Multiple travel plans overlap {start} to {end}."),
            SRC_DATE,
            plans,
        );
    }

    // 6. single ACTIVE plan today
    let active = plans_with_date(plans, &today, &today);
    if active.len() == 1 {
        return Ok(ResolvedPlan {
            plan_id: active[0].plan_id.clone(),
            source: SRC_ACTIVE,
            note: None,
        });
    }
    if active.len() > 1 {
        return Err(format_ambiguous_plan_error(
            &format!("Multiple travel plans are active on {today}."),
            &active,
        ));
    }

    // 7. single UPCOMING plan (anchors endDate >= today OR planning
    //    window endDate >= today).
    let upcoming: Vec<PlanSummary> = plans
        .iter()
        .filter(|plan| {
            if plan.anchors.iter().any(|a| a.end_date.as_str() >= today.as_str()) {
                return true;
            }
            if let Some(window) = &plan.planning_window
                && let Some(we) = window.end_date.as_deref()
                && we >= today.as_str()
            {
                return true;
            }
            false
        })
        .cloned()
        .collect();
    if upcoming.len() == 1 {
        return Ok(ResolvedPlan {
            plan_id: upcoming[0].plan_id.clone(),
            source: SRC_UPCOMING,
            note: None,
        });
    }
    if upcoming.len() > 1 {
        return Err(format_ambiguous_plan_error(
            "Multiple upcoming travel plans exist.",
            &upcoming,
        ));
    }

    // 8. exactly one plan exists
    if plans.len() == 1 {
        return Ok(ResolvedPlan {
            plan_id: plans[0].plan_id.clone(),
            source: SRC_LATEST,
            note: Some("Only one plan exists.".to_string()),
        });
    }
    // 9. most-recently-updated plan
    if plans.len() > 1 {
        let mut sorted: Vec<&PlanSummary> = plans.iter().collect();
        sorted.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        return Ok(ResolvedPlan {
            plan_id: sorted[0].plan_id.clone(),
            source: SRC_LATEST,
            note: Some("No active/upcoming plan exists; using most recently updated plan.".to_string()),
        });
    }

    // 10. no plans
    Err("No travel plans found in DB. Create or seed a plan, then retry.".to_string())
}

/// Read plans from Turso and assemble them into `Vec<PlanSummary>`.
/// Mirrors `listPlans()` (lines 57-79) + `groupPlanRows()`
/// (lines 81-116).
pub async fn list_plans_for_resolver() -> Result<Vec<PlanSummary>, String> {
    let conn = db::connect_read().await?;
    let mut rows = conn
        .query(
            "SELECT pm.plan_id, pm.active_destination, pm.updated_at,
                    da.destination, da.start_date, da.end_date, da.days,
                    p1.status AS p1_status,
                    p1.set_out_date AS p1_set_out_date,
                    p1.return_date AS p1_return_date,
                    p1.duration_days AS p1_duration_days
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

    let mut by_id: std::collections::HashMap<String, PlanSummary> =
        std::collections::HashMap::new();
    let mut order: Vec<String> = Vec::new();

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
        let updated_at: String = row.get(2).unwrap_or_default();
        let dest: String = row.get(3).unwrap_or_default();
        let start: String = row.get(4).unwrap_or_default();
        let end: String = row.get(5).unwrap_or_default();
        let days: i64 = row.get(6).unwrap_or(0);
        let p1_status: String = row.get(7).unwrap_or_default();
        let p1_set_out: Option<String> = row.get(8).ok();
        let p1_return: Option<String> = row.get(9).ok();
        let p1_duration: Option<i64> = row.get(10).ok();

        let summary = by_id.entry(plan_id.clone()).or_insert_with(|| {
            order.push(plan_id.clone());
            PlanSummary {
                plan_id: plan_id.clone(),
                active_destination,
                updated_at,
                anchors: Vec::new(),
                planning_window: None,
            }
        });
        if !dest.is_empty() && !start.is_empty() && !end.is_empty() {
            summary.anchors.push(PlanAnchor {
                destination: dest,
                start_date: start,
                end_date: end,
                days,
            });
        }
        if summary.planning_window.is_none() && !p1_status.is_empty() {
            summary.planning_window = Some(PlanningWindow {
                status: p1_status,
                start_date: p1_set_out.filter(|s| !s.is_empty()),
                end_date: p1_return.filter(|s| !s.is_empty()),
                duration_days: p1_duration,
            });
        }
    }

    let mut plans: Vec<PlanSummary> = order
        .into_iter()
        .filter_map(|id| by_id.remove(&id))
        .collect();

    // Test-only isolation: `resolve-plan` reads EVERY plan in the shared live
    // DB, so a real upcoming/active plan (e.g. an in-progress okinawa-2026)
    // would pollute the precedence ladder of an integration test that seeded
    // its own throwaway plans. When `TRAVEL_RESOLVER_ONLY_PREFIX` is set, scope
    // the resolver to plan_ids carrying that prefix — giving those tests the
    // isolated plan set they were designed around (mirrors the TS test's
    // in-memory PlanSummary[]). Unset in production → no filtering. This is the
    // same test-stub pattern as the `TRAVEL_TODAY` override.
    if let Ok(prefix) = std::env::var("TRAVEL_RESOLVER_ONLY_PREFIX")
        && !prefix.is_empty()
    {
        plans.retain(|p| p.plan_id.starts_with(&prefix));
    }

    Ok(plans)
}

// ---------------------------------------------------------------------------
// CLI subcommand: `travel resolve-plan [args...]`
// ---------------------------------------------------------------------------

/// Parse CLI args (everything after the `resolve-plan` literal).
fn parse_args(args: &[String]) -> Result<ResolveInput, String> {
    parse_args_inner(args, true)
}

/// Lenient variant: ignores unrecognized flags instead of erroring. Used by
/// `resolve_plan_id`, which receives a view command's FULL arg list (e.g.
/// `status --full`, `bookings --dest x`) — flags it doesn't own belong to the
/// caller, not a resolution error.
fn parse_args_lenient(args: &[String]) -> Result<ResolveInput, String> {
    parse_args_inner(args, false)
}

fn parse_args_inner(args: &[String], strict: bool) -> Result<ResolveInput, String> {
    let mut input = ResolveInput::default();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        match a.as_str() {
            "--plan-id" => {
                input.explicit_plan_id = Some(
                    args.get(i + 1)
                        .ok_or_else(|| "missing value for --plan-id".to_string())?
                        .clone(),
                );
                i += 2;
            }
            "--plan-path" => {
                input.plan_path = Some(
                    args.get(i + 1)
                        .ok_or_else(|| "missing value for --plan-path".to_string())?
                        .clone(),
                );
                i += 2;
            }
            "--travel-date" => {
                input.date = Some(
                    args.get(i + 1)
                        .ok_or_else(|| "missing value for --travel-date".to_string())?
                        .clone(),
                );
                i += 2;
            }
            "--travel-start" => {
                input.range_start = Some(
                    args.get(i + 1)
                        .ok_or_else(|| "missing value for --travel-start".to_string())?
                        .clone(),
                );
                i += 2;
            }
            "--travel-end" => {
                input.range_end = Some(
                    args.get(i + 1)
                        .ok_or_else(|| "missing value for --travel-end".to_string())?
                        .clone(),
                );
                i += 2;
            }
            "--help" | "-h" => {
                return Err("__help__".to_string());
            }
            other => {
                if strict {
                    return Err(format!("unknown argument: {other}"));
                }
                // Lenient: skip a flag we don't own. Its value (if any) is left
                // for the caller; we only advance past the flag token itself.
                i += 1;
            }
        }
    }
    // Read $TRAVEL_PLAN_ID env unless explicit --plan-id was given.
    if input.explicit_plan_id.is_none()
        && let Ok(v) = std::env::var("TRAVEL_PLAN_ID")
        && !v.trim().is_empty()
    {
        input.env_plan_id = Some(v);
    }
    Ok(input)
}

/// CLI entry: `travel resolve-plan [args...]`. Prints the resolved
/// plan_id + source + optional note. A debug surface for now.
pub async fn run_cli(args: &[String]) -> Result<(), String> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print_usage();
        return Ok(());
    }
    let mut input = parse_args(args)?;
    // Production `today` is always real — only tests pass a stub.
    if input.today.is_none() {
        input.today = Some(today_iso());
    }
    let plans = list_plans_for_resolver().await?;
    let resolved = resolve_plan_from_summaries(&input, &plans)?;
    println!("plan_id: {}", resolved.plan_id);
    println!("source:  {}", resolved.source);
    if let Some(note) = &resolved.note {
        println!("note:    {note}");
    }
    Ok(())
}

pub fn print_usage() {
    println!(
        "Usage:\n  travel resolve-plan [--plan-id <id> | --plan-path <path> | --travel-date YYYY-MM-DD | --travel-start YYYY-MM-DD --travel-end YYYY-MM-DD]\n\nDebug subcommand: prints the resolved plan_id + source + optional note.\nPlan resolution precedence: explicit --plan-id > $TRAVEL_PLAN_ID > --plan-path > --travel-date > --travel-start/--travel-end > active today > upcoming > most-recent."
    );
}

/// Shared plan-id resolution for view commands (status / itinerary / transport /
/// bookings). Runs the full ladder — explicit --plan-id > $TRAVEL_PLAN_ID >
/// --travel-date / --travel-start/--travel-end > active > upcoming > most-recent
/// — so the view commands behave like the TS CLI (no mandatory TRAVEL_PLAN_ID).
/// Returns the resolved plan_id (hyphen form, e.g. "tokyo-2026").
pub async fn resolve_plan_id(args: &[String]) -> Result<String, String> {
    let mut input = parse_args_lenient(args)?;
    if input.today.is_none() {
        input.today = Some(today_iso());
    }
    let plans = list_plans_for_resolver().await?;
    let resolved = resolve_plan_from_summaries(&input, &plans)?;
    Ok(resolved.plan_id)
}

/// Reject any unrecognized `--flag` in an argument slice, matching the fail-loud
/// parse of set-meals/add-activity/confirm-recommendations/set-route-segment.
/// Without this, a typo'd flag (e.g. `--dry-runn`, `--proven` → `--provven`) is
/// silently ignored — turning a dry-run into a real write, or writing `proven=0`
/// under the wrong flag. `value_flags` each consume the FOLLOWING token as their
/// value (so a value that itself starts with `--` is NOT treated as a flag);
/// `bool_flags` are value-less. Any other `--token` is a hard error. Shared home
/// (colocated with `is_resolver_flag`/`RESOLVER_VALUE_FLAGS`) so every command
/// classifies flags against ONE helper.
pub(crate) fn reject_unknown_flags(
    args: &[String],
    value_flags: &[&str],
    bool_flags: &[&str],
) -> Result<(), String> {
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if value_flags.contains(&a.as_str()) {
            i += 2; // skip the flag AND its value (value may itself start with '--')
        } else if bool_flags.contains(&a.as_str()) {
            i += 1;
        } else if a.starts_with("--") {
            return Err(format!("unknown argument: {a}"));
        } else {
            i += 1; // a value belonging to a preceding value_flag, or a positional
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_resolver_flag_matches_exactly_the_five_selection_flags() {
        // The single source of truth every command parser skips. Locks that it
        // stays in lock-step with the resolver's parse_args_inner arms — a drift
        // here re-introduces the "unknown argument: --travel-date" class of bug.
        for f in [
            "--plan-id",
            "--plan-path",
            "--travel-date",
            "--travel-start",
            "--travel-end",
        ] {
            assert!(is_resolver_flag(f), "{f} must be a resolver flag");
        }
        for f in ["--dest", "--destination", "--force", "--day", "--plan", "-h", "plan-id"] {
            assert!(!is_resolver_flag(f), "{f} must NOT be a resolver flag");
        }
    }

    fn s(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn reject_unknown_flags_errors_on_bogus() {
        let e = reject_unknown_flags(&s(&["--bogus"]), &[], &[]);
        assert!(e.is_err());
        assert!(e.unwrap_err().contains("unknown argument: --bogus"));
    }

    #[test]
    fn reject_unknown_flags_value_flag_consumes_dashdash_value() {
        // A value flag's value may itself start with `--` and must NOT be flagged.
        assert!(reject_unknown_flags(&s(&["--note", "--weird"]), &["--note"], &[]).is_ok());
    }

    #[test]
    fn reject_unknown_flags_bool_flag_does_not_consume_next() {
        // `--dry-run` is value-less, so the following `--bogus` is still unknown.
        assert!(reject_unknown_flags(&s(&["--dry-run", "--bogus"]), &[], &["--dry-run"]).is_err());
    }

    #[test]
    fn reject_unknown_flags_positionals_and_known_pass() {
        assert!(reject_unknown_flags(
            &s(&["zzsrc", "fit", "--proven", "--method", "regex"]),
            &["--method"],
            &["--proven"],
        )
        .is_ok());
    }

    /// Tiny fixture builder. `anchors` is a list of
    /// (destination, start, end) tuples. `window` is an optional
    /// (status, start, end) triple.
    fn fixture(
        plan_id: &str,
        anchors: Vec<(&str, &str, &str)>,
        window: Option<(&str, &str, &str)>,
        updated_at: &str,
    ) -> PlanSummary {
        PlanSummary {
            plan_id: plan_id.to_string(),
            active_destination: format!("{}_2026", plan_id.replace('-', "_")),
            updated_at: updated_at.to_string(),
            anchors: anchors
                .into_iter()
                .map(|(d, s, e)| PlanAnchor {
                    destination: d.to_string(),
                    start_date: s.to_string(),
                    end_date: e.to_string(),
                    days: 0,
                })
                .collect(),
            planning_window: window.map(|(status, s, e)| PlanningWindow {
                status: status.to_string(),
                start_date: Some(s.to_string()),
                end_date: Some(e.to_string()),
                duration_days: None,
            }),
        }
    }

    fn empty_input() -> ResolveInput {
        ResolveInput::default()
    }

    fn input_with_today(today: &str) -> ResolveInput {
        let mut i = ResolveInput::default();
        i.today = Some(today.to_string());
        i
    }

    // 1. explicit
    #[test]
    fn explicit_wins_over_everything() {
        let plans = vec![fixture("tokyo-2026", vec![("tokyo", "2026-02-13", "2026-02-17")], None, "2026-01-01")];
        let mut input = empty_input();
        input.explicit_plan_id = Some("override".to_string());
        input.env_plan_id = Some("env-val".to_string());
        input.date = Some("2026-02-15".to_string());
        let r = resolve_plan_from_summaries(&input, &plans).unwrap();
        assert_eq!(r.plan_id, "override");
        assert_eq!(r.source, SRC_EXPLICIT);
        assert_eq!(r.note, None);
    }

    // 2. env
    #[test]
    fn env_used_when_no_explicit() {
        let plans = vec![fixture("tokyo-2026", vec![("tokyo", "2026-02-13", "2026-02-17")], None, "2026-01-01")];
        let mut input = empty_input();
        input.env_plan_id = Some("tokyo-2026".to_string());
        let r = resolve_plan_from_summaries(&input, &plans).unwrap();
        assert_eq!(r.plan_id, "tokyo-2026");
        assert_eq!(r.source, SRC_ENV);
    }

    // 3. plan-path — derive_plan_id mirrors StateManager.derivePlanId:
    //    data/trips/<X>/ → <X>; anything else → path:<sha1[:12]>.
    #[test]
    fn plan_path_derives_id() {
        let plans = vec![fixture("tokyo-2026", vec![("tokyo", "2026-02-13", "2026-02-17")], None, "2026-01-01")];
        // data/trips/<X>/ path → clean id <X>
        let mut input = empty_input();
        let cwd = std::env::current_dir().unwrap();
        let trips = cwd.join("data/trips/tokyo-2026/plan.md");
        input.plan_path = Some(trips.to_string_lossy().into_owned());
        let r = resolve_plan_from_summaries(&input, &plans).unwrap();
        assert_eq!(r.plan_id, "tokyo-2026");
        assert_eq!(r.source, SRC_PLAN_PATH);

        // non-trips path → hashed id (NOT a slug transform), still source plan-path
        let mut input2 = empty_input();
        input2.plan_path = Some("docs/trips/tokyo_2026.md".to_string());
        let r2 = resolve_plan_from_summaries(&input2, &plans).unwrap();
        assert!(r2.plan_id.starts_with("path:"), "expected path:<hash>, got {}", r2.plan_id);
        assert_eq!(r2.source, SRC_PLAN_PATH);
    }

    // 4. date hit
    #[test]
    fn date_hit_picks_matching_plan() {
        let plans = vec![
            fixture("tokyo-2026", vec![("tokyo", "2026-02-13", "2026-02-17")], None, "2026-01-01"),
            fixture("kyoto-2026", vec![("kyoto", "2026-02-24", "2026-02-28")], None, "2026-01-02"),
        ];
        let mut input = empty_input();
        input.date = Some("2026-02-15".to_string());
        let r = resolve_plan_from_summaries(&input, &plans).unwrap();
        assert_eq!(r.plan_id, "tokyo-2026");
        assert_eq!(r.source, SRC_DATE);
    }

    // 5. date zero match
    #[test]
    fn date_zero_match_throws() {
        let plans = vec![
            fixture("tokyo-2026", vec![("tokyo", "2026-02-13", "2026-02-17")], None, "2026-01-01"),
            fixture("kyoto-2026", vec![("kyoto", "2026-02-24", "2026-02-28")], None, "2026-01-02"),
        ];
        let mut input = empty_input();
        input.date = Some("2026-12-31".to_string());
        let err = resolve_plan_from_summaries(&input, &plans).unwrap_err();
        assert!(err.starts_with("No travel plan contains 2026-12-31."), "{err}");
        assert!(err.contains("tokyo-2026"), "should list plans: {err}");
    }

    // 6. date ambiguous
    #[test]
    fn date_ambiguous_throws_with_list() {
        // Two anchors covering the same date.
        let plans = vec![
            fixture(
                "tokyo-2026",
                vec![
                    ("tokyo", "2026-02-13", "2026-02-17"),
                    ("osaka", "2026-02-17", "2026-02-20"),
                ],
                None,
                "2026-01-01",
            ),
            fixture("kyoto-2026", vec![("kyoto", "2026-02-17", "2026-02-20")], None, "2026-01-02"),
        ];
        let mut input = empty_input();
        input.date = Some("2026-02-18".to_string());
        let err = resolve_plan_from_summaries(&input, &plans).unwrap_err();
        assert!(err.starts_with("Multiple travel plans contain 2026-02-18."), "{err}");
        assert!(err.contains("Use --plan-id <id> or narrow with --travel-date/--travel-start/--travel-end."), "{err}");
        assert!(err.contains("tokyo-2026"), "{err}");
        assert!(err.contains("kyoto-2026"), "{err}");
    }

    // 7. range hit
    #[test]
    fn range_hit_picks_overlapping_plan() {
        let plans = vec![
            fixture("tokyo-2026", vec![("tokyo", "2026-02-13", "2026-02-17")], None, "2026-01-01"),
            fixture("kyoto-2026", vec![("kyoto", "2026-02-24", "2026-02-28")], None, "2026-01-02"),
        ];
        let mut input = empty_input();
        input.range_start = Some("2026-02-13".to_string());
        input.range_end = Some("2026-02-17".to_string());
        let r = resolve_plan_from_summaries(&input, &plans).unwrap();
        assert_eq!(r.plan_id, "tokyo-2026");
        assert_eq!(r.source, SRC_DATE);
    }

    // 8. range zero
    #[test]
    fn range_zero_match_throws() {
        let plans = vec![fixture("tokyo-2026", vec![("tokyo", "2026-02-13", "2026-02-17")], None, "2026-01-01")];
        let mut input = empty_input();
        input.range_start = Some("2030-01-01".to_string());
        input.range_end = Some("2030-01-05".to_string());
        let err = resolve_plan_from_summaries(&input, &plans).unwrap_err();
        assert!(err.starts_with("No travel plan overlaps 2030-01-01 to 2030-01-05."), "{err}");
    }

    // 9. active single
    #[test]
    fn active_single_today() {
        // Today = 2026-06-08 sits inside a future anchor.
        let plans = vec![fixture("trip-a", vec![("osaka", "2026-06-07", "2026-06-10")], None, "2026-05-01")];
        let input = input_with_today("2026-06-08");
        let r = resolve_plan_from_summaries(&input, &plans).unwrap();
        assert_eq!(r.plan_id, "trip-a");
        assert_eq!(r.source, SRC_ACTIVE);
    }

    // 10. active many
    #[test]
    fn active_many_throws_ambiguous() {
        let plans = vec![
            fixture("trip-a", vec![("osaka", "2026-06-07", "2026-06-10")], None, "2026-05-01"),
            fixture("trip-b", vec![("tokyo", "2026-06-08", "2026-06-12")], None, "2026-05-02"),
        ];
        let input = input_with_today("2026-06-08");
        let err = resolve_plan_from_summaries(&input, &plans).unwrap_err();
        assert!(err.starts_with("Multiple travel plans are active on 2026-06-08."), "{err}");
        assert!(err.contains("trip-a") && err.contains("trip-b"), "{err}");
    }

    // 11. upcoming single (today < anchor.endDate but not contained)
    #[test]
    fn upcoming_single_end_date_after_today() {
        // anchor.endDate = 2026-07-15, today = 2026-06-08 — not
        // active (anchor.startDate > today), not past (endDate >=
        // today), so upcoming.
        let plans = vec![fixture("future-trip", vec![("osaka", "2026-07-10", "2026-07-15")], None, "2026-05-01")];
        let input = input_with_today("2026-06-08");
        let r = resolve_plan_from_summaries(&input, &plans).unwrap();
        assert_eq!(r.plan_id, "future-trip");
        assert_eq!(r.source, SRC_UPCOMING);
    }

    // 12. upcoming many
    #[test]
    fn upcoming_many_throws_ambiguous() {
        let plans = vec![
            fixture("future-a", vec![("osaka", "2026-07-10", "2026-07-15")], None, "2026-05-01"),
            fixture("future-b", vec![("tokyo", "2026-08-01", "2026-08-05")], None, "2026-05-02"),
        ];
        let input = input_with_today("2026-06-08");
        let err = resolve_plan_from_summaries(&input, &plans).unwrap_err();
        assert!(err.starts_with("Multiple upcoming travel plans exist."), "{err}");
        assert!(err.contains("future-a") && err.contains("future-b"), "{err}");
    }

    // 13. latest with one plan
    #[test]
    fn latest_with_one_plan_notes_only_one() {
        let plans = vec![fixture("past-trip", vec![("osaka", "2026-01-10", "2026-01-15")], None, "2026-01-01")];
        let input = input_with_today("2026-06-08");
        let r = resolve_plan_from_summaries(&input, &plans).unwrap();
        assert_eq!(r.plan_id, "past-trip");
        assert_eq!(r.source, SRC_LATEST);
        assert_eq!(r.note.as_deref(), Some("Only one plan exists."));
    }

    // 14. latest with multiple past plans picks most-recent
    #[test]
    fn latest_with_many_picks_most_recent_updated_at() {
        let plans = vec![
            fixture("older", vec![("osaka", "2026-01-10", "2026-01-15")], None, "2026-01-01"),
            fixture("newer", vec![("tokyo", "2026-02-10", "2026-02-15")], None, "2026-02-01"),
        ];
        let input = input_with_today("2026-06-08");
        let r = resolve_plan_from_summaries(&input, &plans).unwrap();
        assert_eq!(r.plan_id, "newer");
        assert_eq!(r.source, SRC_LATEST);
        assert_eq!(
            r.note.as_deref(),
            Some("No active/upcoming plan exists; using most recently updated plan.")
        );
    }

    // 15. empty plans
    #[test]
    fn empty_plans_throws() {
        let input = input_with_today("2026-06-08");
        let err = resolve_plan_from_summaries(&input, &[]).unwrap_err();
        assert_eq!(err, "No travel plans found in DB. Create or seed a plan, then retry.");
    }

    // 16. invalid date
    #[test]
    fn invalid_date_throws() {
        let plans = vec![fixture("tokyo-2026", vec![("tokyo", "2026-02-13", "2026-02-17")], None, "2026-01-01")];
        let mut input = empty_input();
        input.date = Some("not-a-date".to_string());
        let err = resolve_plan_from_summaries(&input, &plans).unwrap_err();
        assert_eq!(err, "Invalid date \"not-a-date\". Expected YYYY-MM-DD.");
    }

    // Bonus: window also matches date
    #[test]
    fn date_match_via_planning_window() {
        let plans = vec![fixture(
            "winder",
            vec![],
            Some(("locked", "2026-06-01", "2026-06-30")),
            "2026-05-01",
        )];
        let mut input = empty_input();
        input.date = Some("2026-06-15".to_string());
        let r = resolve_plan_from_summaries(&input, &plans).unwrap();
        assert_eq!(r.plan_id, "winder");
        assert_eq!(r.source, SRC_DATE);
    }

    // Bonus: is_iso_date
    #[test]
    fn iso_date_helper() {
        assert!(is_iso_date(Some("2026-06-08")));
        assert!(!is_iso_date(Some("2026-6-8")));
        assert!(!is_iso_date(Some("2026/06/08")));
        assert!(!is_iso_date(None));
    }

    // Bonus: format_plan_list produces a sensible table
    #[test]
    fn format_plan_list_smoke() {
        let plans = vec![fixture("tokyo-2026", vec![("tokyo", "2026-02-13", "2026-02-17")], None, "2026-01-01")];
        let s = format_plan_list_for_resolver(&plans);
        assert!(s.contains("PLAN_ID          STATUS    ACTIVE_DEST      DATE_ANCHORS"));
        assert!(s.contains("tokyo-2026"));
        assert!(s.contains("tokyo: 2026-02-13..2026-02-17"));
    }

    // Bonus: empty list table
    #[test]
    fn format_plan_list_empty() {
        let s = format_plan_list_for_resolver(&[]);
        assert_eq!(s, "No travel plans found.");
    }

    // Bonus: derive_plan_id strips extension and swaps _ -> -
    #[test]
    fn derive_plan_id_data_trips_match() {
        // ^data/trips/<X>/ → <X> (relative to cwd). Use an absolute cwd-rooted path
        // so canonicalize-fallback yields a deterministic relative path.
        let cwd = std::env::current_dir().unwrap();
        let p = cwd.join("data/trips/tokyo-2026/plan.md");
        assert_eq!(derive_plan_id(p.to_str().unwrap()), "tokyo-2026");
    }

    #[test]
    fn derive_plan_id_non_trips_is_hashed() {
        // Anything not under data/trips/<X>/ → path:<sha1[:12]>, NOT a slug transform.
        let id = derive_plan_id("tokyo_2026.md");
        assert!(id.starts_with("path:"), "expected path:<hash>, got {id}");
        assert_eq!(id.len(), 5 + 12); // "path:" + 12 hex chars
        // Deterministic for the same input.
        assert_eq!(derive_plan_id("tokyo_2026.md"), id);
    }
}
