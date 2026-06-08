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
    if let Some(sources) = validate_ota_sources(&mut issues).await {
        validate_claude_md_consistency(&sources, &mut issues);
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
    }

    emit_report(&issues);
    Ok(())
}

fn project_root() -> PathBuf {
    // The binary's CWD is the project root in normal usage. Resolve relative
    // file paths against CWD to match the TS behavior (which uses __dirname/..).
    if let Ok(cwd) = std::env::current_dir() {
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
    supported: bool,
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
        let status: String = row.get(2).unwrap_or_default();
        let _scraper_script: String = row.get(3).unwrap_or_default();
        let _notes: String = row.get(4).unwrap_or_default();

        let supported = status == "active";
        source_ids.push(source_id.clone());
        sources.sources.insert(
            source_id.clone(),
            OtaSource {
                source_id,
                // The TS hardcodes currency to 'TWD' for every source in this
                // simplified validation path (the JSON-backed field is gone
                // post-de-JSON; we mirror that here).
                currency: "TWD".to_string(),
                supported,
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

// --- DB check: CLAUDE.md <-> ota_sources consistency ---

#[derive(Default, Debug)]
struct ClaudeOtaEntry {
    source_id: String,
    supported: String,
}

fn parse_claude_ota_table(content: &str, issues: &mut Vec<Issue>) -> Vec<ClaudeOtaEntry> {
    // The TS uses a non-consuming lookahead to bound the table at the next
    // section break; the Rust `regex` crate does not support look-around, so
    // we consume the terminator (an extra non-pipe line is harmless because
    // the row filter below only keeps lines starting with `|`).
    let re = match regex::Regex::new(
        r"\| Source ID \| Name \| Type \| Supported \| Scraper \|[\s\S]*?(?:\n\n|\n###|\n##|\z)",
    ) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    let table_match = match re.find(content) {
        Some(m) => m.as_str(),
        None => {
            issues.push(Issue {
                category: "claude-md".to_string(),
                severity: Severity::Warning,
                message: "Could not find OTA Sources table in CLAUDE.md".to_string(),
                file: Some("CLAUDE.md".to_string()),
                line: None,
            });
            return Vec::new();
        }
    };

    let mut entries: Vec<ClaudeOtaEntry> = Vec::new();
    for line in table_match.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with('|') {
            continue;
        }
        let cells: Vec<String> = trimmed
            .split('|')
            .map(|c| c.trim().to_string())
            .filter(|c| !c.is_empty())
            .collect();
        if cells.len() < 5 {
            continue;
        }
        // Skip header row + separator row.
        if cells[0] == "Source ID" || cells[0].starts_with("---") {
            continue;
        }
        entries.push(ClaudeOtaEntry {
            source_id: cells[0].replace('`', ""),
            supported: cells[3].clone(),
        });
    }
    entries
}

fn validate_claude_md_consistency(sources: &OtaSourcesFile, issues: &mut Vec<Issue>) {
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

    let entries = parse_claude_ota_table(&content, issues);
    if entries.is_empty() {
        return;
    }

    for entry in &entries {
        let source = match sources.sources.get(&entry.source_id) {
            Some(s) => s,
            None => {
                issues.push(Issue {
                    category: "consistency".to_string(),
                    severity: Severity::Warning,
                    message: format!(
                        "CLAUDE.md lists \"{}\" but not in Turso ota_sources",
                        entry.source_id
                    ),
                    file: Some("CLAUDE.md".to_string()),
                    line: None,
                });
                continue;
            }
        };

        let json_supported = source.supported;
        let md_supported = entry.supported.contains('✅');
        let md_scrape_only = entry.supported.contains("scrape-only");

        if json_supported && !md_supported && !md_scrape_only {
            issues.push(Issue {
                category: "consistency".to_string(),
                severity: Severity::Error,
                message: format!(
                    "{}: Turso ota_sources says supported=true but CLAUDE.md shows unsupported",
                    entry.source_id
                ),
                file: Some("CLAUDE.md".to_string()),
                line: None,
            });
        }
        if !json_supported && md_supported && !md_scrape_only {
            issues.push(Issue {
                category: "consistency".to_string(),
                severity: Severity::Error,
                message: format!(
                    "{}: Turso ota_sources says supported=false but CLAUDE.md shows ✅",
                    entry.source_id
                ),
                file: Some("CLAUDE.md".to_string()),
                line: None,
            });
        }
    }

    let claude_ids: HashSet<&str> = entries.iter().map(|e| e.source_id.as_str()).collect();
    let mut db_ids: Vec<&str> = sources.sources.keys().map(String::as_str).collect();
    db_ids.sort();
    for id in db_ids {
        if !claude_ids.contains(id) {
            issues.push(Issue {
                category: "consistency".to_string(),
                severity: Severity::Info,
                message: format!(
                    "{id}: in Turso ota_sources but not listed in CLAUDE.md OTA table"
                ),
                file: Some("CLAUDE.md".to_string()),
                line: None,
            });
        }
    }
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
                    "{slug}: no rows in destination_areas. Run npx ts-node scripts/seed-destination-refs.ts."
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
            message: "airlines table is empty. Run npx ts-node scripts/seed-ota-knowledge.ts.".to_string(),
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
        ("hotel_areas", "scripts/backfill-local-reference-data.ts"),
        ("transport_routes", "scripts/backfill-local-reference-data.ts"),
        ("transport_hubs", "scripts/backfill-local-reference-data.ts"),
        ("destination_areas", "scripts/seed-destination-refs.ts"),
        ("airlines", "scripts/seed-ota-knowledge.ts"),
        (
            "shaping_research_artifacts",
            "scripts/backfill-local-reference-data.ts",
        ),
        (
            "shaping_selected_offers",
            "scripts/backfill-local-reference-data.ts",
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
            issues.push(Issue {
                category: "turso-reference".to_string(),
                severity: Severity::Error,
                message: format!(
                    "Turso {label} has no rows. Run npx ts-node {seed}."
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
    validate_iso_date(start, "start date")?;
    validate_iso_date(end, "end date")?;
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

/// Port of validateIsoDate(input, fieldName): required → format (YYYY-MM-DD) →
/// real-date validity. Matches the TS error strings exactly.
fn validate_iso_date(input: &str, field: &str) -> Result<(), String> {
    if input.is_empty() {
        return Err(format!("{field} is required"));
    }
    // ^(\d{4})-(\d{2})-(\d{2})$
    let bytes = input.as_bytes();
    let well_formed = input.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && input[0..4].bytes().all(|b| b.is_ascii_digit())
        && input[5..7].bytes().all(|b| b.is_ascii_digit())
        && input[8..10].bytes().all(|b| b.is_ascii_digit());
    if !well_formed {
        return Err(format!(
            "{field} must be YYYY-MM-DD format (got: \"{input}\")"
        ));
    }
    // Real calendar date? (e.g. 2026-13-99 is well-formed but invalid.)
    if NaiveDate::parse_from_str(input, "%Y-%m-%d").is_err() {
        return Err(format!("{field} is not a valid date: \"{input}\""));
    }
    Ok(())
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
