// `travel validate data` and `travel doctor` — data consistency + project
// health checks. Ports scripts/validate-data.ts. Read-only, plain-text.
//
// Output format must match the TS baseline byte-for-byte. Sections are emitted
// in fixed order (Errors → Warnings → Info → Summary). Each issue line is
//   2 spaces + icon + 1 space (errors have no trailing space, warnings/info have
//   one extra after the emoji-with-VS-16) + `[category] message (file:line)`
//
// Two TS checks (validateDependencies, validateCliScripts) are ported as no-op
// stubs to keep parity with the current test baselines; the project is
// migrating away from npm and root package.json will be deleted, so these will
// be removed at that point.

use crate::db;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Validate,
    Doctor,
}

struct Issue {
    category: String,
    severity: Severity,
    message: String,
    file: Option<String>,
    line: Option<usize>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Severity {
    Error,
    Warning,
    Info,
}

pub async fn run(mode: Mode) -> Result<(), String> {
    let label = match mode {
        Mode::Validate => "Data consistency validation",
        Mode::Doctor => "Project health check (doctor)",
    };
    println!("Running {label}...\n");

    let mut issues: Vec<Issue> = Vec::new();

    // Always-run: data consistency checks
    if validate_ota_sources(&mut issues).await.is_some() {
        validate_ota_coverage(&mut issues).await;
        validate_claude_md_consistency(&mut issues);
    }
    validate_python_scripts(&mut issues);
    validate_destinations(&mut issues).await;
    validate_holiday_calendars(&mut issues).await;
    validate_reference_tables(&mut issues).await;

    // Always-run: documentation ↔ code drift checks
    validate_completed_items(&mut issues);
    validate_skill_files(&mut issues);
    validate_cli_scripts(&mut issues);

    // Doctor mode would add environment readiness checks. validateDependencies
    // (node_modules / npm binaries) is intentionally a no-op here — the project
    // is migrating away from npm, and the current TS test baseline shows both
    // modes produce identical output. Remove when root package.json is deleted.
    if matches!(mode, Mode::Doctor) {
        // no-op: validateDependencies() (intentionally skipped — see note above)
        // Agent-first map-link check: cross-country (ocean-spanning) Maps legs
        // across all plans are doctor errors. Ambiguous-stop info/warnings stay
        // advisory (surfaced only by `validate-itinerary`, not doctor).
        validate_map_links_all_plans(&mut issues).await;
        // Map-snapshot staleness: dashboard map PNGs that no longer match the
        // itinerary (advisory warning, never an error — re-run snapshot-maps).
        validate_maps_fresh_all_plans(&mut issues).await;
        validate_maps_completeness_all_plans(&mut issues).await;
    }

    emit_report(&issues);
    Ok(())
}

/// Doctor-only: run the map-link lint across every plan and add cross-country
/// errors. Best-effort — skips silently if the plan list can't be read.
async fn validate_map_links_all_plans(issues: &mut Vec<Issue>) {
    let Ok(conn) = db::connect_read().await else {
        return;
    };
    let mut rows = match conn
        .query("SELECT plan_id FROM plan_metadata ORDER BY plan_id", ())
        .await
    {
        Ok(r) => r,
        Err(_) => return,
    };
    let mut plan_ids: Vec<String> = Vec::new();
    while let Ok(Some(row)) = rows.next().await {
        if let Ok(id) = row.get::<String>(0) {
            plan_ids.push(id);
        }
    }
    for plan_id in plan_ids {
        for (day, message) in crate::validate_itinerary::map_link_errors(&plan_id).await {
            issues.push(Issue {
                category: "map-links".to_string(),
                severity: Severity::Error,
                message: format!("{message} (day {day})"),
                file: Some(format!("plan:{plan_id}")),
                line: None,
            });
        }
        for (day, message) in crate::validate_itinerary::malformed_map_link_warnings(&plan_id).await {
            issues.push(Issue {
                category: "map-links".to_string(),
                severity: Severity::Warning,
                message: format!("{message} (day {day})"),
                file: Some(format!("plan:{plan_id}")),
                line: None,
            });
        }
        // Reservation gate (advisory, not pass/fail): sit-down restaurants not yet
        // enrolled in the booking ledger. One concise actionable line per plan —
        // self-clears as each is booked. Keeps doctor's error/warning counts clean.
        let unbooked = crate::validate_itinerary::unbooked_reservations(&plan_id).await;
        if !unbooked.is_empty() {
            let days: Vec<String> = unbooked.iter().map(|(d, _)| format!("day {d}")).collect();
            issues.push(Issue {
                category: "reservations".to_string(),
                severity: Severity::Info,
                message: format!(
                    "{} restaurant(s) may need a reservation, not yet tracked ({}). Enroll: add-activity + set-activity-booking … pending; walk-in spots can be left as-is.",
                    unbooked.len(),
                    days.join(", ")
                ),
                file: Some(format!("plan:{plan_id}")),
                line: None,
            });
        }
    }
}

/// Doctor-only: surface plans whose static dashboard map PNGs are stale relative
/// to the latest itinerary edit. Advisory only — emitted as warnings, never
/// errors (a stale map doesn't break data integrity; it just needs a re-snapshot).
/// Best-effort — skips silently if the plan list can't be read.
async fn validate_maps_fresh_all_plans(issues: &mut Vec<Issue>) {
    let Ok(conn) = db::connect_read().await else {
        return;
    };
    let mut rows = match conn
        .query(
            "SELECT plan_id FROM plans WHERE deleted_at IS NULL ORDER BY plan_id",
            (),
        )
        .await
    {
        Ok(r) => r,
        Err(_) => return,
    };
    let mut plan_ids: Vec<String> = Vec::new();
    while let Ok(Some(row)) = rows.next().await {
        if let Ok(id) = row.get::<String>(0) {
            plan_ids.push(id);
        }
    }
    for plan_id in plan_ids {
        let verdict = match crate::check_maps_fresh::evaluate(&conn, &plan_id).await {
            Ok(v) => v,
            Err(_) => continue,
        };
        match verdict {
            crate::check_maps_fresh::Status::NeverSnapshotted => {
                issues.push(Issue {
                    category: "maps-fresh".to_string(),
                    severity: Severity::Warning,
                    message: format!(
                        "maps never snapshotted — run scripts/snapshot-maps.sh {plan_id} <dest>"
                    ),
                    file: Some(format!("plan:{plan_id}")),
                    line: None,
                });
            }
            crate::check_maps_fresh::Status::Stale { snapshotted_at } => {
                issues.push(Issue {
                    category: "maps-fresh".to_string(),
                    severity: Severity::Warning,
                    message: format!(
                        "itinerary changed since maps snapshotted ({snapshotted_at}) — maps STALE, re-run scripts/snapshot-maps.sh"
                    ),
                    file: Some(format!("plan:{plan_id}")),
                    line: None,
                });
            }
            crate::check_maps_fresh::Status::Fresh { .. } => {}
        }
    }
}

/// Doctor-only: surface plans whose map-artifact manifest is missing or has
/// EMPTY keys. Advisory only — emitted as warnings, never errors.
async fn validate_maps_completeness_all_plans(issues: &mut Vec<Issue>) {
    let Ok(conn) = db::connect_read().await else {
        return;
    };
    let mut rows = match conn
        .query(
            "SELECT plan_id FROM plans WHERE deleted_at IS NULL ORDER BY plan_id",
            (),
        )
        .await
    {
        Ok(r) => r,
        Err(_) => return,
    };
    let mut plan_ids: Vec<String> = Vec::new();
    while let Ok(Some(row)) = rows.next().await {
        if let Ok(id) = row.get::<String>(0) {
            plan_ids.push(id);
        }
    }
    for plan_id in plan_ids {
        let verdict = match crate::check_maps_fresh::evaluate_completeness(&conn, &plan_id).await {
            Ok(v) => v,
            Err(_) => continue,
        };
        match verdict {
            crate::check_maps_fresh::CompletenessVerdict::NoManifest => {
                issues.push(Issue {
                    category: "maps-complete".to_string(),
                    severity: Severity::Warning,
                    message: "no map manifest — run snapshot-maps".to_string(),
                    file: Some(format!("plan:{plan_id}")),
                    line: None,
                });
            }
            crate::check_maps_fresh::CompletenessVerdict::Incomplete { line } => {
                issues.push(Issue {
                    category: "maps-complete".to_string(),
                    severity: Severity::Warning,
                    message: line,
                    file: Some(format!("plan:{plan_id}")),
                    line: None,
                });
            }
            crate::check_maps_fresh::CompletenessVerdict::Complete { .. } => {}
        }
    }
}

fn project_root() -> PathBuf {
    // Normal CLI usage starts at the repo root. Integration tests may run the
    // binary from the crate directory, so walk upward until the repo marker is found.
    if let Ok(cwd) = std::env::current_dir() {
        for dir in cwd.ancestors() {
            if dir.join("CLAUDE.md").exists() {
                return dir.to_path_buf();
            }
        }
        cwd
    } else {
        PathBuf::from(".")
    }
}

fn project_file(rel: &str) -> PathBuf {
    project_root().join(rel)
}

fn file_exists(rel: &str) -> bool {
    project_file(rel).exists()
}

fn read_file(rel: &str) -> Option<String> {
    fs::read_to_string(project_file(rel)).ok()
}

// --- DB check: ota_sources ---

#[derive(Default)]
struct OtaSource {
    source_id: String,
    currency: String,
}

#[derive(Default)]
struct OtaSourcesFile {
    sources: HashMap<String, OtaSource>,
}

async fn validate_ota_sources(issues: &mut Vec<Issue>) -> Option<OtaSourcesFile> {
    let conn = db::connect_read().await.ok()?;

    let mut rows = match conn
        .query(
            "SELECT source_id, name, status, scraper_script, notes FROM ota_sources ORDER BY source_id",
            (),
        )
        .await
    {
        Ok(r) => r,
        Err(err) => {
            issues.push(Issue {
                category: "ota-sources".to_string(),
                severity: Severity::Error,
                message: format!("failed to query ota_sources: {err}"),
                file: Some("turso:ota_sources".to_string()),
                line: None,
            });
            return None;
        }
    };

    let mut sources = OtaSourcesFile::default();
    let mut source_ids: Vec<String> = Vec::new();

    while let Some(row) = match rows.next().await {
        Ok(r) => r,
        Err(err) => {
            issues.push(Issue {
                category: "ota-sources".to_string(),
                severity: Severity::Error,
                message: format!("failed to read ota_sources row: {err}"),
                file: Some("turso:ota_sources".to_string()),
                line: None,
            });
            return None;
        }
    } {
        let source_id: String = row.get(0).unwrap_or_default();
        let _name: String = row.get(1).unwrap_or_default();
        let _status: String = row.get(2).unwrap_or_default();
        let _scraper_script: String = row.get(3).unwrap_or_default();
        let _notes: String = row.get(4).unwrap_or_default();

        source_ids.push(source_id.clone());
        sources.sources.insert(
            source_id.clone(),
            OtaSource {
                source_id,
                // The TS hardcodes currency to 'TWD' for every source in this
                // simplified validation path (the JSON-backed field is gone
                // post-de-JSON; we mirror that here).
                currency: "TWD".to_string(),
            },
        );
    }

    if sources.sources.is_empty() {
        issues.push(Issue {
            category: "ota-sources".to_string(),
            severity: Severity::Error,
            message: "Turso ota_sources has no rows. Run npm run db:migrate:turso.".to_string(),
            file: Some("turso:ota_sources".to_string()),
            line: None,
        });
        return None;
    }

    // Read types from normalized child table (kept for parity with the TS
    // pre-de-JSON shape; not currently consumed by the checks but the source
    // record is still populated for future use).
    let mut type_rows = match conn
        .query(
            "SELECT source_id, type FROM ota_source_types ORDER BY source_id",
            (),
        )
        .await
    {
        Ok(r) => r,
        Err(_) => return Some(sources),
    };
    let mut types_by_source: HashMap<String, Vec<String>> = HashMap::new();
    while let Some(row) = type_rows.next().await.ok().flatten() {
        let sid: String = row.get(0).unwrap_or_default();
        let typ: String = row.get(1).unwrap_or_default();
        types_by_source.entry(sid).or_default().push(typ);
    }
    let _ = types_by_source; // types are loaded but the parity output does not surface them

    for id in &source_ids {
        let source = match sources.sources.get(id) {
            Some(s) => s,
            None => continue,
        };

        if source.source_id != *id {
            issues.push(Issue {
                category: "ota-sources".to_string(),
                severity: Severity::Error,
                message: format!(
                    "Source ID mismatch: key \"{id}\" vs source_id \"{}\"",
                    source.source_id
                ),
                file: Some("turso:ota_sources".to_string()),
                line: None,
            });
        }

        // scraper_script is DECOMMISSIONED — see the TS counterpart for context.

        let valid_currencies = ["TWD", "JPY", "USD", "EUR"];
        if !valid_currencies.contains(&source.currency.as_str()) {
            issues.push(Issue {
                category: "ota-sources".to_string(),
                severity: Severity::Warning,
                message: format!("{id}: unknown currency \"{}\"", source.currency),
                file: Some("turso:ota_sources".to_string()),
                line: None,
            });
        }
    }

    Some(sources)
}

// --- DB check: OTA coverage + CLAUDE.md pointer ---

async fn validate_ota_coverage(issues: &mut Vec<Issue>) {
    let Ok(conn) = db::connect_read().await else {
        return;
    };

    let mut missing = match conn
        .query(
            "SELECT s.source_id \
             FROM ota_sources s \
             LEFT JOIN ota_source_coverage c ON c.source_id = s.source_id \
             WHERE s.status = 'active' \
             GROUP BY s.source_id \
             HAVING COUNT(c.source_id) = 0 \
             ORDER BY s.source_id",
            (),
        )
        .await
    {
        Ok(r) => r,
        Err(err) => {
            issues.push(Issue {
                category: "ota-coverage".to_string(),
                severity: Severity::Error,
                message: format!("failed to query ota_source_coverage: {err}"),
                file: Some("turso:ota_source_coverage".to_string()),
                line: None,
            });
            return;
        }
    };
    while let Some(row) = missing.next().await.ok().flatten() {
        let source_id: String = row.get(0).unwrap_or_default();
        issues.push(Issue {
            category: "ota-coverage".to_string(),
            severity: Severity::Error,
            message: format!("{source_id}: active ota_source has no coverage row"),
            file: Some("turso:ota_source_coverage".to_string()),
            line: None,
        });
    }

    for (sql, message_prefix) in [
        (
            "SELECT c.source_id, c.product_type \
             FROM ota_source_coverage c \
             LEFT JOIN product_types p ON p.code = c.product_type \
             WHERE p.code IS NULL \
             ORDER BY c.source_id, c.product_type",
            "unknown product_type",
        ),
        (
            "SELECT c.source_id, c.blocked_reason_code \
             FROM ota_source_coverage c \
             LEFT JOIN coverage_block_reasons b ON b.code = c.blocked_reason_code \
             WHERE c.blocked_reason_code IS NOT NULL AND b.code IS NULL \
             ORDER BY c.source_id, c.blocked_reason_code",
            "unknown blocked_reason_code",
        ),
    ] {
        let Ok(mut rows) = conn.query(sql, ()).await else {
            continue;
        };
        while let Some(row) = rows.next().await.ok().flatten() {
            let source_id: String = row.get(0).unwrap_or_default();
            let value: String = row.get(1).unwrap_or_default();
            issues.push(Issue {
                category: "ota-coverage".to_string(),
                severity: Severity::Error,
                message: format!("{source_id}: {message_prefix} \"{value}\""),
                file: Some("turso:ota_source_coverage".to_string()),
                line: None,
            });
        }
    }
}

fn validate_claude_md_consistency(issues: &mut Vec<Issue>) {
    let content = match read_file("CLAUDE.md") {
        Some(c) => c,
        None => {
            issues.push(Issue {
                category: "claude-md".to_string(),
                severity: Severity::Error,
                message: "CLAUDE.md not found".to_string(),
                file: Some("CLAUDE.md".to_string()),
                line: None,
            });
            return;
        }
    };

    const POINTER: &str =
        "Provider coverage is DB data — run `travel ota-status` (catalog edited via `travel set-ota-*`).";
    if !content.contains(POINTER) {
        issues.push(Issue {
            category: "claude-md".to_string(),
            severity: Severity::Error,
            message: "CLAUDE.md OTA section must point to `travel ota-status`".to_string(),
            file: Some("CLAUDE.md".to_string()),
            line: None,
        });
    }

    let section = content
        .split("## OTA Sources")
        .nth(1)
        .and_then(|rest| rest.split("\n## ").next())
        .unwrap_or("");
    if section.contains("| Source ID |") || section.contains("| `") {
        issues.push(Issue {
            category: "claude-md".to_string(),
            severity: Severity::Error,
            message: "CLAUDE.md OTA section must not re-encode per-source status rows".to_string(),
            file: Some("CLAUDE.md".to_string()),
            line: None,
        });
    }
    for forbidden in ["PROVEN REAL", "DEFERRED", "renderer-wedge", "cloudflare", "captcha"] {
        if section.contains(forbidden) {
            issues.push(Issue {
                category: "claude-md".to_string(),
                severity: Severity::Error,
                message: format!(
                    "CLAUDE.md OTA section must not encode status fact \"{forbidden}\""
                ),
                file: Some("CLAUDE.md".to_string()),
                line: None,
            });
        }
    }

    // Trip-provenance drift guard (T2 / P2, 2026-07-02): CLAUDE.md must not re-assert the FALSE
    // claim that okinawa-2026 was created via shaping-adopt. The DB record shows it was NOT
    // (shaping-20260525-093508: 0 candidates adopted; no shaping-adopt in operation_runs). Matches
    // the FULL false phrase only — `shaping-adopt` / "adopted"/"Originated from" appear LEGITIMATELY
    // elsewhere (the shaping-adopt command docs), so a bare-substring check would false-positive.
    for msg in okinawa_false_provenance_violations(&content) {
        issues.push(Issue {
            category: "claude-md".to_string(),
            severity: Severity::Error,
            message: msg,
            file: Some("CLAUDE.md".to_string()),
            line: None,
        });
    }

    // Planning-flow routing consistency (T3/T4/T5, 2026-07-02): the trip-intake router + Stage-2
    // modes must be present and must use the flow_decision.rs vocabulary (so docs can't drift from
    // the command). One narrow block, not per-phrase brittleness (Codex-advised minimal oracle).
    for msg in planning_flow_doc_violations(&content) {
        issues.push(Issue {
            category: "claude-md".to_string(),
            severity: Severity::Error,
            message: msg,
            file: Some("CLAUDE.md".to_string()),
            line: None,
        });
    }
}

/// Pure matcher for the planning-flow routing docs (T3/T4/T5). Returns one message per missing
/// required element. Asserts the intake router + known-flights fast-path + Stage-2 modes are present
/// and use the flow_decision.rs vocabulary (`shaping skip --reason known_flights`; modes
/// `shop`/`ingest-known`/`defer`; validation mandatory). Kept narrow + testable.
fn planning_flow_doc_violations(content: &str) -> Vec<String> {
    let mut out = Vec::new();
    if !content.contains("Trip-intake router") {
        out.push("CLAUDE.md must contain the \"Trip-intake router\" classification table".to_string());
    }
    // known-flights fast-path must name its flow-decision record (ties docs to the shipped command).
    if !content.contains("flow-decision shaping skip --reason known_flights") {
        out.push(
            "CLAUDE.md trip-intake router must name the known-flights record \
             `flow-decision shaping skip --reason known_flights`"
                .to_string(),
        );
    }
    // Stage-2 modes must all appear + validation-mandatory stated (must match flow_decision.rs MODES).
    for mode in ["shop", "ingest-known", "defer"] {
        if !content.contains(mode) {
            out.push(format!("CLAUDE.md must name Stage-2 mode `{mode}` (flow_decision.rs MODES)"));
        }
    }
    if !content.contains("VALIDATION is mandatory") {
        out.push(
            "CLAUDE.md Stage-2 modes must state \"VALIDATION is mandatory in every mode\"".to_string(),
        );
    }
    out
}

/// Pure matcher for the okinawa false-provenance drift (T2). Returns one message per FULL false
/// phrase found. Phrase-specific (not bare `adopt`/`shaping-adopt`, which are legit elsewhere) so it
/// can be unit-tested without touching real docs.
fn okinawa_false_provenance_violations(content: &str) -> Vec<String> {
    let mut out = Vec::new();
    if content.contains("Originated from the `shaping-20260525-093508`") {
        out.push(
            "CLAUDE.md falsely claims okinawa-2026 \"Originated from the `shaping-20260525-093508`\" \
             run — the DB shows 0 candidates adopted, no shaping-adopt. State it was built from \
             pre-decided flights/hotel entered directly."
                .to_string(),
        );
    }
    if content.contains("`shaping-20260525-093508` run was adopted into") {
        out.push(
            "CLAUDE.md falsely claims the `shaping-20260525-093508` run \"was adopted into\" \
             okinawa-2026 — it was exploratory only (0 adopted). Correct the provenance."
                .to_string(),
        );
    }
    out
}

// --- Filesystem check: Python scripts ---

fn validate_python_scripts(issues: &mut Vec<Issue>) {
    let dir = project_file("scripts");
    if !dir.is_dir() {
        return;
    }
    let entries = match fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    // The TS uses `(?!\s*\.)` (negative lookahead) which the Rust `regex`
    // crate does not support. We pre-compile a simpler match pattern and
    // perform the "next char is not a dot" check manually.
    let price_x2 = regex::Regex::new(r"price\s*\*\s*2").unwrap();
    let rate_left = regex::Regex::new(r"[\d.]+\s*\*\s*3[12]\.?\d*").unwrap();
    let rate_right = regex::Regex::new(r"3[12]\.?\d*\s*\*\s*[\d.]+").unwrap();
    let currency = regex::Regex::new(r#"["']TWD["']|["']USD["']|["']JPY["']"#).unwrap();

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("py") {
            continue;
        }
        let file_name = match path.file_name().and_then(|s| s.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        let rel = format!("scripts/{file_name}");
        let content = match read_file(&rel) {
            Some(c) => c,
            None => continue,
        };

        for (i, line) in content.lines().enumerate() {
            let line_num = i + 1;

            if let Some(m) = price_x2.find(line) {
                let after = &line[m.end()..];
                let has_dot = after.trim_start().starts_with('.');
                if !has_dot && !line.contains('#') {
                    issues.push(Issue {
                        category: "hardcoded".to_string(),
                        severity: Severity::Error,
                        message: "Hardcoded pax: \"price * 2\" should use pax variable".to_string(),
                        file: Some(rel.clone()),
                        line: Some(line_num),
                    });
                }
            }

            if (rate_left.is_match(line) || rate_right.is_match(line))
                && !line.contains('#')
                && !line.contains("rate")
            {
                issues.push(Issue {
                    category: "hardcoded".to_string(),
                    severity: Severity::Warning,
                    message: "Possible hardcoded exchange rate".to_string(),
                    file: Some(rel.clone()),
                    line: Some(line_num),
                });
            }

            if currency.is_match(line) && !line.contains("default") && !line.contains('=') && !line.contains('#') {
                issues.push(Issue {
                    category: "hardcoded".to_string(),
                    severity: Severity::Info,
                    message: "Hardcoded currency string (consider parameterizing)".to_string(),
                    file: Some(rel.clone()),
                    line: Some(line_num),
                });
            }
        }
    }
}

// --- DB check: destinations ---

async fn validate_destinations(issues: &mut Vec<Issue>) {
    let conn = match db::connect_read().await {
        Ok(c) => c,
        Err(err) => {
            issues.push(Issue {
                category: "destinations".to_string(),
                severity: Severity::Error,
                message: format!("failed to connect to Turso: {err}"),
                file: Some("turso:destination_config".to_string()),
                line: None,
            });
            return;
        }
    };

    let mut rows = match conn
        .query("SELECT slug FROM destination_config ORDER BY slug", ())
        .await
    {
        Ok(r) => r,
        Err(err) => {
            issues.push(Issue {
                category: "destinations".to_string(),
                severity: Severity::Error,
                message: format!("failed to query destination_config: {err}"),
                file: Some("turso:destination_config".to_string()),
                line: None,
            });
            return;
        }
    };

    let mut slugs: Vec<String> = Vec::new();
    while let Some(row) = match rows.next().await {
        Ok(r) => r,
        Err(err) => {
            issues.push(Issue {
                category: "destinations".to_string(),
                severity: Severity::Error,
                message: format!("failed to read destination_config row: {err}"),
                file: Some("turso:destination_config".to_string()),
                line: None,
            });
            return;
        }
    } {
        let slug: String = row.get(0).unwrap_or_default();
        slugs.push(slug);
    }

    if slugs.is_empty() {
        issues.push(Issue {
            category: "destinations".to_string(),
            severity: Severity::Error,
            message: "Turso destination_config has no rows. Run npm run db:migrate:turso.".to_string(),
            file: Some("turso:destination_config".to_string()),
            line: None,
        });
        return;
    }

    let mut area_rows = match conn
        .query(
            "SELECT slug, COUNT(*) AS n FROM destination_areas GROUP BY slug",
            (),
        )
        .await
    {
        Ok(r) => r,
        Err(err) => {
            issues.push(Issue {
                category: "destinations".to_string(),
                severity: Severity::Error,
                message: format!("failed to query destination_areas: {err}"),
                file: Some("turso:destination_areas".to_string()),
                line: None,
            });
            return;
        }
    };
    let mut count_by_slug: HashMap<String, i64> = HashMap::new();
    while let Some(row) = area_rows.next().await.ok().flatten() {
        let slug: String = row.get(0).unwrap_or_default();
        let n: i64 = row.get(1).unwrap_or(0);
        count_by_slug.insert(slug, n);
    }

    for slug in &slugs {
        let n = count_by_slug.get(slug).copied().unwrap_or(0);
        if n == 0 {
            issues.push(Issue {
                category: "destinations".to_string(),
                severity: Severity::Warning,
                message: format!(
                    "{slug}: no rows in destination_areas. Run ./bin/travel db seed destination-refs."
                ),
                file: Some("turso:destination_areas".to_string()),
                line: None,
            });
        }
    }

    let mut airline_rows = match conn.query("SELECT COUNT(*) AS n FROM airlines", ()).await {
        Ok(r) => r,
        Err(err) => {
            issues.push(Issue {
                category: "destinations".to_string(),
                severity: Severity::Error,
                message: format!("failed to query airlines: {err}"),
                file: Some("turso:airlines".to_string()),
                line: None,
            });
            return;
        }
    };
    let n_airlines: i64 = match airline_rows.next().await {
        Ok(Some(row)) => row.get(0).unwrap_or(0),
        _ => 0,
    };
    if n_airlines == 0 {
        issues.push(Issue {
            category: "destinations".to_string(),
            severity: Severity::Error,
            message: "airlines table is empty. Run ./bin/travel db seed ota-knowledge.".to_string(),
            file: Some("turso:airlines".to_string()),
            line: None,
        });
    }
}

// --- DB check: holiday calendars ---

async fn validate_holiday_calendars(issues: &mut Vec<Issue>) {
    let conn = match db::connect_read().await {
        Ok(c) => c,
        Err(err) => {
            issues.push(Issue {
                category: "holidays".to_string(),
                severity: Severity::Error,
                message: format!("failed to connect to Turso: {err}"),
                file: Some("turso:holidays".to_string()),
                line: None,
            });
            return;
        }
    };

    let mut rows = match conn
        .query(
            "SELECT country, year, COUNT(*) AS day_count, \
             SUM(CASE WHEN is_holiday = 2 AND name IS NOT NULL THEN 1 ELSE 0 END) AS named_holidays, \
             MIN(source_url) AS source_url, \
             MIN(fetched_at) AS fetched_at \
             FROM holidays GROUP BY country, year ORDER BY country, year",
            (),
        )
        .await
    {
        Ok(r) => r,
        Err(err) => {
            issues.push(Issue {
                category: "holidays".to_string(),
                severity: Severity::Error,
                message: format!("failed to query holidays: {err}"),
                file: Some("turso:holidays".to_string()),
                line: None,
            });
            return;
        }
    };

    let mut found_any = false;
    while let Some(row) = rows.next().await.ok().flatten() {
        found_any = true;
        let country: String = row.get(0).unwrap_or_default();
        let year: i64 = row.get(1).unwrap_or(0);
        let day_count: i64 = row.get(2).unwrap_or(0);
        let named_holidays: i64 = row.get(3).unwrap_or(0);
        let source_url: Option<String> = row.get(4).ok();
        let fetched_at: Option<String> = row.get(5).ok();
        let year_str = year.to_string();

        if day_count < 365 {
            issues.push(Issue {
                category: "holidays".to_string(),
                severity: Severity::Warning,
                message: format!(
                    "{country} {year_str}: only {day_count} day rows in Turso"
                ),
                file: Some("turso:holidays".to_string()),
                line: None,
            });
        }
        if named_holidays == 0 {
            issues.push(Issue {
                category: "holidays".to_string(),
                severity: Severity::Error,
                message: format!("{country} {year_str}: no named holidays in Turso"),
                file: Some("turso:holidays".to_string()),
                line: None,
            });
        }
        let url_empty = source_url.as_deref().map(str::is_empty).unwrap_or(true);
        let fetched_empty = fetched_at.as_deref().map(str::is_empty).unwrap_or(true);
        if url_empty || fetched_empty {
            issues.push(Issue {
                category: "holidays".to_string(),
                severity: Severity::Error,
                message: format!(
                    "{country} {year_str}: missing source_url/fetched_at provenance"
                ),
                file: Some("turso:holidays".to_string()),
                line: None,
            });
        }
    }

    if !found_any {
        issues.push(Issue {
            category: "holidays".to_string(),
            severity: Severity::Error,
            message: "Turso holidays has no rows. Run npm run db:migrate:turso and npm run db:fetch:holidays:tw -- <DGPA CSV URL>.".to_string(),
            file: Some("turso:holidays".to_string()),
            line: None,
        });
    }
}

// --- DB check: reference tables ---

async fn validate_reference_tables(issues: &mut Vec<Issue>) {
    let conn = match db::connect_read().await {
        Ok(c) => c,
        Err(err) => {
            issues.push(Issue {
                category: "turso-reference".to_string(),
                severity: Severity::Error,
                message: format!("failed to connect to Turso: {err}"),
                file: None,
                line: None,
            });
            return;
        }
    };

    let checks: [(&str, &str); 7] = [
        ("hotel_areas", "no live seeder (archived backfill-local-reference-data.ts)"),
        ("transport_routes", "no live seeder (archived backfill-local-reference-data.ts)"),
        ("transport_hubs", "no live seeder (archived backfill-local-reference-data.ts)"),
        ("destination_areas", "./bin/travel db seed destination-refs"),
        ("airlines", "./bin/travel db seed ota-knowledge"),
        (
            "shaping_research_artifacts",
            "populated by shaping-import (no standalone seeder)",
        ),
        (
            "shaping_selected_offers",
            "populated by shaping-adopt (no standalone seeder)",
        ),
    ];

    for (label, seed) in checks {
        let sql = format!("SELECT COUNT(*) AS count FROM {label}");
        let mut rows = match conn.query(&sql, ()).await {
            Ok(r) => r,
            Err(err) => {
                issues.push(Issue {
                    category: "turso-reference".to_string(),
                    severity: Severity::Error,
                    message: format!("failed to query {label}: {err}"),
                    file: Some(format!("turso:{label}")),
                    line: None,
                });
                continue;
            }
        };
        let n: i64 = match rows.next().await {
            Ok(Some(row)) => row.get(0).unwrap_or(0),
            _ => 0,
        };
        if n == 0 {
            let action = if seed.starts_with("./bin/") {
                format!("Run {seed}.")
            } else {
                format!("{seed}.")
            };
            issues.push(Issue {
                category: "turso-reference".to_string(),
                severity: Severity::Error,
                message: format!(
                    "Turso {label} has no rows. {action}"
                ),
                file: Some(format!("turso:{label}")),
                line: None,
            });
        }
    }
}

// --- Filesystem check: completed items in CLAUDE.md ---

fn validate_completed_items(issues: &mut Vec<Issue>) {
    let content = match read_file("CLAUDE.md") {
        Some(c) => c,
        None => return,
    };
    let line_re = match regex::Regex::new(r"^- ✅ .+$") {
        Ok(r) => r,
        Err(_) => return,
    };
    let path_re = match regex::Regex::new(r"`((?:src|scripts|data|tests)/[^`]+\.[a-z]+)`") {
        Ok(r) => r,
        Err(_) => return,
    };

    for line_match in line_re.find_iter(&content) {
        let matched_line = line_match.as_str();
        // Recompute the 1-based line number from the match position.
        let prefix = &content[..line_match.start()];
        let line_num = prefix.bytes().filter(|b| *b == b'\n').count() + 1;

        for path_match in path_re.find_iter(matched_line) {
            let file_path = path_match
                .as_str()
                .trim_start_matches('`')
                .trim_end_matches('`');
            if !file_exists(file_path) {
                issues.push(Issue {
                    category: "completed-items".to_string(),
                    severity: Severity::Error,
                    message: format!("Completed item references missing file: {file_path}"),
                    file: Some("CLAUDE.md".to_string()),
                    line: Some(line_num),
                });
            }
        }
    }
}

// --- Filesystem check: skill SKILL.md files ---

fn validate_skill_files(issues: &mut Vec<Issue>) {
    let content = match read_file("CLAUDE.md") {
        Some(c) => c,
        None => return,
    };
    let skill_path_re = match regex::Regex::new(r"`(src/skills/[^`]+/SKILL\.md)`") {
        Ok(r) => r,
        Err(_) => return,
    };
    let mut checked: HashSet<String> = HashSet::new();
    for m in skill_path_re.find_iter(&content) {
        let path = m
            .as_str()
            .trim_start_matches('`')
            .trim_end_matches('`')
            .to_string();
        if !checked.insert(path.clone()) {
            continue;
        }
        if !file_exists(&path) {
            issues.push(Issue {
                category: "skill-files".to_string(),
                severity: Severity::Error,
                message: format!("Skill file not found: {path}"),
                file: Some("CLAUDE.md".to_string()),
                line: None,
            });
        }
    }

    let skills_dir = project_file("src/skills");
    if skills_dir.is_dir()
        && let Ok(entries) = fs::read_dir(&skills_dir)
    {
        for entry in entries.flatten() {
            let dir_name = match entry.file_name().into_string() {
                Ok(n) => n,
                Err(_) => continue,
            };
            let skill_md = format!("src/skills/{dir_name}/SKILL.md");
            if file_exists(&skill_md) && !checked.contains(&skill_md) {
                issues.push(Issue {
                    category: "skill-files".to_string(),
                    severity: Severity::Warning,
                    message: format!("Skill exists but not listed in CLAUDE.md: {skill_md}"),
                    file: None,
                    line: None,
                });
            }
        }
    }
}

// --- Skipped check: validateCliScripts (no-op stub) ---
//
// Mirrors the TS validateCliScripts() function, which extracts ts-node source
// paths from package.json scripts. The project is migrating away from npm and
// root package.json is slated for deletion, so this check is a no-op for now.
// Remove when the root npm script layer is gone.
fn validate_cli_scripts(_issues: &mut Vec<Issue>) {
    // intentionally empty — see comment above.
}

// --- Output rendering ---

fn emit_report(issues: &[Issue]) {
    let errors: Vec<&Issue> = issues.iter().filter(|i| i.severity == Severity::Error).collect();
    let warnings: Vec<&Issue> = issues.iter().filter(|i| i.severity == Severity::Warning).collect();
    let infos: Vec<&Issue> = issues.iter().filter(|i| i.severity == Severity::Info).collect();

    if !errors.is_empty() {
        println!("## Errors\n");
        for r in &errors {
            println!("  ❌ [{}] {}{}", r.category, r.message, format_loc(r));
        }
        println!();
    }
    if !warnings.is_empty() {
        println!("## Warnings\n");
        for r in &warnings {
            println!("  ⚠️  [{}] {}{}", r.category, r.message, format_loc(r));
        }
        println!();
    }
    if !infos.is_empty() {
        println!("## Info\n");
        for r in &infos {
            println!("  ℹ️  [{}] {}{}", r.category, r.message, format_loc(r));
        }
        println!();
    }

    println!("## Summary\n");
    println!("  Errors:   {}", errors.len());
    println!("  Warnings: {}", warnings.len());
    println!("  Info:     {}", infos.len());

    if !errors.is_empty() {
        std::process::exit(1);
    }
}

fn format_loc(r: &Issue) -> String {
    match (&r.file, r.line) {
        (Some(f), Some(line)) => format!(" ({f}:{line})"),
        (Some(f), None) => format!(" ({f})"),
        _ => String::new(),
    }
}

// ============================================================================
// Date Range Validation (for set-dates command)
// ============================================================================
// Mirrors src/types/validation.ts validateDateRange exactly.
// Error messages must be byte-identical for CLI parity.

use chrono::NaiveDate;

/// Validate date range (start, end) and return days on success.
/// Faithful port of validateDateRange (src/types/validation.ts), which calls
/// validateIsoDate(field) for each date then checks start<=end. Error TEXT must be
/// byte-identical to TS (verified against live `set-dates` output). NO max-days cap —
/// TS has none (a 425-day range is valid).
pub fn validate_date_range(start: &str, end: &str) -> Result<u32, String> {
    crate::checks::validate_iso_date(start, "start date")?;
    crate::checks::validate_iso_date(end, "end date")?;
    // Both are valid ISO dates here.
    let start_date = NaiveDate::parse_from_str(start, "%Y-%m-%d").unwrap();
    let end_date = NaiveDate::parse_from_str(end, "%Y-%m-%d").unwrap();
    if start_date > end_date {
        return Err(format!(
            "Start date ({start}) cannot be after end date ({end})"
        ));
    }
    // TS: Math.ceil((end-start)/day) + 1. For midnight ISO dates this is the
    // inclusive day count.
    let days = (end_date - start_date).num_days() as u32 + 1;
    Ok(days)
}

// NOTE: validate_iso_date now lives in crate::checks (single source of truth);
// validate_date_range calls crate::checks::validate_iso_date. The error strings
// are unchanged (still byte-identical to the TS originals).

#[cfg(test)]
mod okinawa_provenance_tests {
    use super::*;

    #[test]
    fn flags_false_originated_from_phrase() {
        let bad = "okinawa-2026 (Originated from the `shaping-20260525-093508` Shaping run.)";
        assert_eq!(okinawa_false_provenance_violations(bad).len(), 1);
    }

    #[test]
    fn flags_false_adopted_into_phrase() {
        let bad = "the `shaping-20260525-093508` run was adopted into **`okinawa-2026`**";
        assert_eq!(okinawa_false_provenance_violations(bad).len(), 1);
    }

    #[test]
    fn corrected_wording_passes() {
        let good = "Flights/hotel were pre-decided and entered directly via `set-flight`/`set-hotel`; \
                    the `shaping-20260525-093508` run was exploratory only — 0 candidates adopted.";
        assert!(okinawa_false_provenance_violations(good).is_empty());
    }

    #[test]
    fn legit_shaping_adopt_command_docs_do_not_false_positive() {
        // These are the LEGITIMATE uses elsewhere in CLAUDE.md — must NOT trip the guard.
        let legit = "./bin/travel shaping-adopt <candidate_id> <plan_id> --create-plan --dest <slug>\n\
                     adopted research-first staged planning model\n\
                     For a freshly created plan (e.g. `shaping-adopt`) pass version_before=0.";
        assert!(okinawa_false_provenance_violations(legit).is_empty());
    }
}

#[cfg(test)]
mod planning_flow_doc_tests {
    use super::*;

    #[test]
    fn complete_routing_docs_pass() {
        let good = "### Trip-intake router\n\
            | known flights | ... | flow-decision shaping skip --reason known_flights |\n\
            Stage 2 modes: shop | ingest-known | defer. transport/accommodation VALIDATION is mandatory in every mode.";
        assert!(planning_flow_doc_violations(good).is_empty());
    }

    #[test]
    fn missing_router_flagged() {
        let bad = "Stage 2 modes: shop | ingest-known | defer. VALIDATION is mandatory in every mode.\n\
                   flow-decision shaping skip --reason known_flights";
        let v = planning_flow_doc_violations(bad);
        assert!(v.iter().any(|m| m.contains("Trip-intake router")), "{v:?}");
    }

    #[test]
    fn missing_a_mode_flagged() {
        let bad = "### Trip-intake router\nflow-decision shaping skip --reason known_flights\n\
                   modes: shop | defer. VALIDATION is mandatory in every mode.";
        let v = planning_flow_doc_violations(bad);
        assert!(v.iter().any(|m| m.contains("ingest-known")), "{v:?}");
    }
}

#[cfg(test)]
mod date_range_tests {
    use super::*;

    #[test]
    fn valid_ranges() {
        assert_eq!(validate_date_range("2026-02-13", "2026-02-17").unwrap(), 5);
        assert_eq!(validate_date_range("2026-06-15", "2026-06-20").unwrap(), 6);
        // No 365 cap in TS — a 425-day range is valid.
        assert_eq!(validate_date_range("2026-01-01", "2027-03-01").unwrap(), 425);
    }

    #[test]
    fn start_after_end_has_dates_in_message() {
        assert_eq!(
            validate_date_range("2026-03-05", "2026-03-01"),
            Err("Start date (2026-03-05) cannot be after end date (2026-03-01)".to_string())
        );
    }

    #[test]
    fn bad_format_message() {
        assert_eq!(
            validate_date_range("2026/03/01", "2026-03-05"),
            Err("start date must be YYYY-MM-DD format (got: \"2026/03/01\")".to_string())
        );
    }

    #[test]
    fn invalid_date_message() {
        assert_eq!(
            validate_date_range("2026-13-99", "2026-03-05"),
            Err("start date is not a valid date: \"2026-13-99\"".to_string())
        );
    }

    #[test]
    fn empty_required_message() {
        assert_eq!(
            validate_date_range("", "2026-03-05"),
            Err("start date is required".to_string())
        );
    }
}
