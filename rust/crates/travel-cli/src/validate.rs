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
use libsql::params;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;

#[derive(Clone, PartialEq, Eq)]
pub enum Mode {
    Validate,
    Doctor,
    Publish {
        plan_id: String,
        dest: Option<String>,
    },
}

struct Issue {
    category: String,
    severity: Severity,
    message: String,
    file: Option<String>,
    line: Option<usize>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Severity {
    Error,
    Warning,
    Info,
}

/// Trip-record in-text markers (spec §5 lint): the emoji/Chinese markers agents
/// used to write into titles/notes before the record_* columns existed. English
/// words ("visited"/"skipped") are deliberately NOT matched — they false-positive
/// on legitimate English content (Fable#15). Keyword list is CODE (the allowed
/// "keywords matched against data" class), never i18n chrome.
pub(crate) fn record_marker_hits(text: &str) -> bool {
    const MARKERS: &[&str] = &["✅", "⏭", "實際行程", "實際用餐", "未前往"];
    MARKERS.iter().any(|m| text.contains(m))
}

/// WARN when trip-record markers are still written as TEXT in itinerary
/// content (titles, meal texts, route notes, session focus, activity notes).
/// The record is data (record_status / record_note / record_summary_zh), not
/// prose — the lint points at every row that should become a `record-*` call.
async fn validate_record_markers(issues: &mut Vec<Issue>) {
    const QUERY: &str = "\
        SELECT 'activities.title' AS src, plan_id, day_number, session_type, title AS text \
        FROM activities WHERE title IS NOT NULL \
        UNION ALL SELECT 'activities.notes', plan_id, day_number, session_type, notes \
        FROM activities WHERE notes IS NOT NULL \
        UNION ALL SELECT 'session_meals.meal', plan_id, day_number, session_type, meal \
        FROM session_meals WHERE meal IS NOT NULL \
        UNION ALL SELECT 'day_route_segments.notes', plan_id, day_number, NULL, notes \
        FROM day_route_segments WHERE notes IS NOT NULL \
        UNION ALL SELECT 'timesofday.focus', plan_id, day_number, session_type, focus \
        FROM timesofday WHERE focus IS NOT NULL \
        UNION ALL SELECT 'timesofday.focus_zh', plan_id, day_number, session_type, focus_zh \
        FROM timesofday WHERE focus_zh IS NOT NULL";
    let Ok(conn) = db::connect_read().await else {
        return; // no creds: the other checks already report the connection problem
    };
    let Ok(mut rows) = conn.query(QUERY, params![]).await else {
        return; // record columns not migrated yet — nothing to lint against
    };
    let mut seen: Vec<(String, String, String, String)> = Vec::new();
    while let Ok(Some(row)) = rows.next().await {
        let src: String = row.get(0).unwrap_or_default();
        let plan_id: String = row.get(1).unwrap_or_default();
        let day: i64 = row.get(2).unwrap_or(0);
        let session: Option<String> = row.get(3).ok().flatten();
        let text: String = row.get(4).unwrap_or_default();
        if record_marker_hits(&text) {
            seen.push((src, plan_id, day.to_string(), session.unwrap_or_default()));
        }
    }
    for (src, plan_id, day, session) in seen {
        issues.push(Issue {
            category: "record-markers".to_string(),
            severity: Severity::Warning,
            message: format!(
                "{src} still carries an in-text trip-record marker (✅/⏭/實際行程/實際用餐/未前往) — \
                 move it to the record columns (record-activity/record-meal/record-leg/record-session) \
                 [plan {plan_id} day {day} {session}]"
            ),
            file: None,
            line: None,
        });
    }
}

/// Audit H2 (data half): `validate data` rules over the trip-record columns
/// (spec 2026-11-16-trip-record-design.md §4/§5). Row-level records
/// (`record_status`) and session summary records (`record_summary_zh`, a
/// complete record on its own) are both checked.
/// - Rule 1 (ERROR): `record_status` must be NULL | 'done' | 'skipped'. The CLI
///   guards its own writes with the `RecordStatus` enum; raw `db exec` does not,
///   so the lint is the only net under the whole table family.
/// - Rule 2 (NOT IMPLEMENTED, deliberately): `record_summary_zh` set on a
///   session with no row-level records is NOT warned — spec §5 `record-session`
///   writes the summary ALONE as a complete session-level record (with its own
///   audit triad), so a summary without row statuses is valid data.
/// - Rule 2b (WARN): a record — status row or summary — on a VARIANT day
///   (`days.variant_of`): alternatives are not recorded (spec §5), so any such
///   row is stray content.
/// - Rule 3 (WARN): a record — status row or summary — whose day is dated after
///   the destination-local today is future content marked as actual. The CLI
///   refuses future days (record.rs); raw `db exec` does not. "Today" comes
///   from the shared `record::destination_today` helper (spec K3#9) — never
///   the UTC `today_iso`.
async fn validate_record_columns(issues: &mut Vec<Issue>) {
    let push = |issues: &mut Vec<Issue>, severity: Severity, message: String| {
        issues.push(Issue {
            category: "record-columns".to_string(),
            severity,
            message,
            file: None,
            line: None,
        });
    };
    let Ok(conn) = db::connect_read().await else {
        return; // no creds: the other checks already report the connection problem
    };
    let rows = match travel_db::repo::records::recorded_rows(&conn).await {
        Ok(rows) => rows,
        Err(e) if e.contains("no such column") => {
            push(
                issues,
                Severity::Error,
                format!(
                    "cannot check record columns: record_status/record_note are missing — \
                     run ./bin/travel db migrate ({e})"
                ),
            );
            return;
        }
        Err(e) => {
            push(issues, Severity::Error, format!("cannot check record columns: {e}"));
            return;
        }
    };
    // Rule 1 — illegal status values (pure rule, applies to every destination).
    for message in invalid_record_status_lines(&rows) {
        push(issues, Severity::Error, message);
    }
    // Rule 1b — records on variant days (spec §5: alternatives are not recorded).
    for message in variant_recorded_lines(&rows) {
        push(issues, Severity::Warning, message);
    }
    // Rule 3 — future-dated records, one destination-local today per destination
    // (first-seen order keeps the report deterministic). A destination whose
    // local today cannot be computed is an ERROR, not a silent skip — matches
    // the fail-loud precedent of validate_booking_integrity.
    let mut dest_order: Vec<String> = Vec::new();
    for r in &rows {
        if !dest_order.contains(&r.destination) {
            dest_order.push(r.destination.clone());
        }
    }
    for dest in &dest_order {
        match crate::record::destination_today(&conn, dest).await {
            Ok(today) => {
                for message in future_recorded_lines(&rows, dest, &today) {
                    push(issues, Severity::Warning, message);
                }
            }
            Err(e) => push(
                issues,
                Severity::Error,
                format!("cannot check future-dated records for destination {dest}: {e}"),
            ),
        }
    }
}

/// Pure rule (audit H2, rule 1): every non-NULL `record_status` must be exactly
/// 'done' or 'skipped' (case-sensitive — the CLI enum writes only these). One
/// ERROR line per offending row, naming plan/day/session/row. Summary rows
/// (`timesofday`, no `record_status`) are out of scope here.
fn invalid_record_status_lines(rows: &[travel_db::repo::records::RecordLintRow]) -> Vec<String> {
    rows.iter()
        .filter(|r| r.status.as_deref().is_some_and(|s| s != "done" && s != "skipped"))
        .map(|r| {
            record_row_at(r)
                + &format!(
                    ": {} 「{}」 has record_status '{}' — legal values are done | skipped | NULL \
                     (fix the row, or clear it with travel clear-record)",
                    r.table,
                    r.row_text,
                    r.status.as_deref().unwrap_or_default()
                )
        })
        .collect()
}

/// Pure rule (audit H2, rule 3): a record dated after the destination-local
/// `today` — row-level records AND session summaries (a summary alone is a
/// complete record, spec §5) — is future content marked as actual: one WARN
/// line per row. Only rows of `destination` are evaluated (each destination has
/// its own local today). A row with no day date (missing/blank `days.date`)
/// also warns: the date check cannot run on it, and a silent skip would hide it.
fn future_recorded_lines(
    rows: &[travel_db::repo::records::RecordLintRow],
    destination: &str,
    today: &str,
) -> Vec<String> {
    rows.iter()
        .filter(|r| r.destination == destination)
        .filter_map(|r| {
            let at = record_row_at(r);
            let date = r.date.as_deref().map(str::trim).filter(|d| !d.is_empty());
            match date {
                // Same comparison as record.rs's day guard: ISO string compare.
                Some(date) if date > today => Some(format!(
                    "{at}: {} is recorded but its day is dated {date}, after the \
                     destination-local today {today} — records are for what already happened",
                    record_what(r)
                )),
                None => Some(format!(
                    "{at}: {} is recorded but its day has no date (no matching days row) \
                     — cannot check that it is not in the future",
                    record_what(r)
                )),
                _ => None,
            }
        })
        .collect()
}

/// Pure rule (audit H2, spec §5): a record — row-level status or session
/// summary — sitting on a VARIANT day (`days.variant_of` set) cannot be an
/// actual-trip record: alternatives are not recorded (`record-*` refuses them,
/// spec §5), so any such row is stray content from raw `db exec`. One WARN line
/// per row, naming plan/day/session/row and the variant_of link.
fn variant_recorded_lines(rows: &[travel_db::repo::records::RecordLintRow]) -> Vec<String> {
    rows.iter()
        .filter(|r| r.variant_of.is_some())
        .map(|r| {
            record_row_at(r)
                + &format!(
                    ": {} is recorded but day {} is a variant day (variant_of {}) — \
                     alternatives are not recorded; clear the record or drop the variant day",
                    record_what(r),
                    r.day,
                    r.variant_of.unwrap_or_default()
                )
        })
        .collect()
}

/// The "what was recorded" clause shared by the record-column issue lines:
/// `activities 「東福寺」` for row-level records, `session summary 「…」` for
/// `timesofday.record_summary_zh` records.
fn record_what(r: &travel_db::repo::records::RecordLintRow) -> String {
    if r.is_summary {
        format!("session summary 「{}」", r.row_text)
    } else {
        format!("{} 「{}」", r.table, r.row_text)
    }
}

/// The `plan <id> (<dest>) day <n> <session>` locator shared by the
/// record-column issue lines.
fn record_row_at(r: &travel_db::repo::records::RecordLintRow) -> String {
    format!(
        "plan {} ({}) day {}{}",
        r.plan_id,
        r.destination,
        r.day,
        r.session
            .as_deref()
            .map(|s| format!(" {s}"))
            .unwrap_or_default()
    )
}

pub async fn run(mode: Mode) -> Result<(), String> {
    if let Mode::Publish { plan_id, dest } = mode {
        return run_publish(&plan_id, dest.as_deref()).await;
    }

    let label = match mode {
        Mode::Validate => "Data consistency validation",
        Mode::Doctor => "Project health check (doctor)",
        _ => unreachable!("Publish handled above"),
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
    validate_omiyage(&mut issues).await;
    validate_domestic_accommodations(&mut issues).await;
    validate_stay_placeholders(&mut issues).await;
    validate_activity_geo(&mut issues).await;
    validate_clothing_tips(&mut issues).await;
    validate_destination_timezones(&mut issues).await;
    validate_reference_tables(&mut issues).await;
    validate_booking_integrity(&mut issues).await;
    validate_record_markers(&mut issues).await;
    validate_record_columns(&mut issues).await;
    validate_day_variants(&mut issues).await;
    validate_plan_documents(&mut issues).await;
    validate_fit_booking(&mut issues).await;
    validate_process_status_ladders(&mut issues).await;

    // Always-run: documentation ↔ code drift checks
    validate_completed_items(&mut issues);
    validate_skill_files(&mut issues);
    validate_cli_scripts(&mut issues);

    // Doctor mode would add environment readiness checks. validateDependencies
    // (node_modules / npm binaries) is intentionally a no-op here — the project
    // is migrating away from npm, and the current TS test baseline shows both
    // modes produce identical output. Remove when root package.json is deleted.
    if mode == Mode::Doctor {
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

fn empty_timezone_warnings(rows: &[(String, Option<String>)]) -> Vec<String> {
    rows.iter().filter(|(_, tz)| tz.as_deref().is_none_or(|tz| tz.trim().is_empty()))
        .map(|(slug, _)| format!("{slug}: destination_config.timezone is empty"))
        .collect()
}

async fn validate_destination_timezones(issues: &mut Vec<Issue>) {
    let Ok(conn) = db::connect_read().await else { return; };
    let mut rows = match conn.query("SELECT slug, timezone FROM destination_config", ()).await {
        Ok(rows) => rows,
        Err(e) => {
            issues.push(Issue { category: "destination-timezone".into(), severity: Severity::Error,
                message: format!("failed to query destination_config.timezone: {e}"),
                file: Some("turso:destination_config".into()), line: None });
            return;
        }
    };
    let mut timezones = Vec::new();
    loop {
        match rows.next().await {
            Ok(Some(row)) => timezones.push((row.get::<String>(0).unwrap_or_default(), row.get::<Option<String>>(1).unwrap_or(None))),
            Ok(None) => break,
            Err(e) => {
                issues.push(Issue { category: "destination-timezone".into(), severity: Severity::Error,
                    message: format!("failed to read destination_config.timezone: {e}"),
                    file: Some("turso:destination_config".into()), line: None });
                return;
            }
        }
    }
    for message in empty_timezone_warnings(&timezones) {
        issues.push(Issue { category: "destination-timezone".into(), severity: Severity::Warning,
            message, file: Some("turso:destination_config".into()), line: None });
    }
}

#[cfg(test)]
mod timezone_tests {
    use super::*;
    #[test]
    fn empty_timezone_warnings_include_slug_and_column() {
        let got = empty_timezone_warnings(&[("kyoto_2026".into(), None), ("x".into(), Some("".into())), ("ok".into(), Some("Asia/Taipei".into()))]);
        assert_eq!(got.len(), 2);
        assert!(got[0].contains("kyoto_2026") && got[0].contains("destination_config.timezone"));
        assert!(got[1].contains("x") && got[1].contains("destination_config.timezone"));
    }
}

#[cfg(test)]
mod record_marker_tests {
    use super::record_marker_hits;

    #[test]
    fn hits_emoji_and_chinese_markers() {
        assert!(record_marker_hits("✅ 實際行程：東福寺"));
        assert!(record_marker_hits("⏭ 未前往"));
        assert!(record_marker_hits("中午 ✅ 實際用餐"));
        assert!(record_marker_hits("未前往：京極スタンド"));
    }

    #[test]
    fn english_words_do_not_hit() {
        assert!(!record_marker_hits("visited Toji"));
        assert!(!record_marker_hits("skipped"));
        assert!(!record_marker_hits("Actual: ramen"));
        assert!(!record_marker_hits(""));
    }
}

/// One text location scanned by the stay-placeholder lint (A3): a route-segment
/// endpoint or an activity title, with the plan+destination's P4 status.
#[derive(Debug, Clone)]
pub(crate) struct StayPlaceholderRow {
    pub src: &'static str,
    pub plan_id: String,
    pub day: i64,
    pub session: Option<String>,
    pub text: String,
    /// P4 (process_4_accommodation) status; empty when the plan has no P4 row.
    pub p4_status: String,
}

/// Stay-placeholder keywords (A3): the undecided-stay markers the convention
/// (routing.md 國內行程) writes into overnight endpoints/titles while the stay
/// is NOT yet decided. Keyword list is CODE (the allowed "keywords matched
/// against data" class), same as [`record_marker_hits`].
pub(crate) fn stay_placeholder_hits(text: &str) -> bool {
    const MARKERS: &[&str] = &["住宿待定", "住宿未定"];
    MARKERS.iter().any(|m| text.contains(m))
}

/// Pure rule behind the A3 WARN: only a plan whose P4 is booked/confirmed has a
/// DECIDED stay — placeholder text there is stale (the booked pin renders
/// beside the 頭城（住宿待定） stop). selecting/pending/no-P4 rows are the
/// convention working as intended and emit nothing.
pub(crate) fn stay_placeholder_lines(rows: &[StayPlaceholderRow]) -> Vec<String> {
    rows.iter()
        .filter(|r| {
            matches!(r.p4_status.as_str(), "booked" | "confirmed") && stay_placeholder_hits(&r.text)
        })
        .map(|r| {
            format!(
                "{src} still carries a stay placeholder (住宿待定/住宿未定) but P4 \
                 (process_4_accommodation) is {p4} — the stay is decided; replace the \
                 placeholder with the booked stay (set-route-segment / derive-routes for \
                 segment endpoints, update-activity for activity titles) \
                 [plan {plan} day {day} {session}]",
                src = r.src,
                p4 = r.p4_status,
                plan = r.plan_id,
                day = r.day,
                session = r.session.as_deref().unwrap_or("-")
            )
        })
        .collect()
}

#[cfg(test)]
mod stay_placeholder_tests {
    use super::*;

    fn row(src: &'static str, text: &str, p4: &str) -> StayPlaceholderRow {
        StayPlaceholderRow {
            src,
            plan_id: "zz-test-plan".into(),
            day: 2,
            session: None,
            text: text.into(),
            p4_status: p4.into(),
        }
    }

    #[test]
    fn hits_both_placeholder_markers_not_plain_stay_words() {
        assert!(stay_placeholder_hits("頭城（住宿待定）"));
        assert!(stay_placeholder_hits("抵達頭城（住宿未定，也可能當天回）"));
        // A booked/normal stay mention is NOT a placeholder.
        assert!(!stay_placeholder_hits("住宿：晏京"));
        assert!(!stay_placeholder_hits("入住頭城飯店"));
        assert!(!stay_placeholder_hits(""));
    }

    #[test]
    fn warns_only_when_p4_is_booked_or_confirmed() {
        let hit = |p4: &str| {
            stay_placeholder_lines(&[row("day_route_segments.to_place", "頭城（住宿待定）", p4)]).len()
        };
        assert_eq!(hit("booked"), 1, "booked → WARN");
        assert_eq!(hit("confirmed"), 1, "confirmed counts as booked");
        assert_eq!(hit("selecting"), 0, "undecided stay → the convention, not a defect");
        assert_eq!(hit("pending"), 0);
        assert_eq!(hit(""), 0, "no P4 row yet → nothing to compare");
    }

    #[test]
    fn message_names_location_and_fix_commands() {
        let mut r = row("activities.title", "抵達頭城（住宿未定，也可能當天回）", "booked");
        r.session = Some("afternoon".into());
        let lines = stay_placeholder_lines(&[r]);
        assert_eq!(lines.len(), 1);
        let m = &lines[0];
        assert!(m.contains("activities.title"), "{m}");
        assert!(m.contains("zz-test-plan") && m.contains("day 2") && m.contains("afternoon"), "{m}");
        assert!(m.contains("set-route-segment") && m.contains("update-activity"), "{m}");
        // A non-placeholder text on a booked plan emits nothing.
        assert!(stay_placeholder_lines(&[row("day_route_segments.from_place", "頭城", "booked")]).is_empty());
    }
}

/// One domestic_accommodations row as seen by the price-basis lints (spec §3):
/// destination + id + hotel + ranking + price_source + price_basis.
#[derive(Debug, Clone)]
pub(crate) struct AccommodationBasisRow {
    pub destination: String,
    pub id: String,
    pub hotel: String,
    pub ranking: Option<i64>,
    pub price_source: Option<String>,
    pub price_basis: Option<String>,
}

/// Pure rule: a basis value that is neither NULL nor incl/excl is dirty (the
/// column has no CHECK — raw `db exec` or old code can write junk). One WARN
/// per dirty row, naming the id and the value.
pub(crate) fn invalid_basis_line(r: &AccommodationBasisRow) -> Option<String> {
    match r.price_basis.as_deref() {
        None => None,
        Some("incl") | Some("excl") => None,
        Some(v) => Some(format!(
            "{dest}/{hotel} ({id}) price_basis '{v}' is not incl or excl — the column takes only \
             incl (含稅) / excl (未稅); fix via update-accommodation --id {id} --price-basis incl|excl|clear",
            dest = r.destination,
            hotel = r.hotel,
            id = r.id
        )),
    }
}

/// Pure rule: an INFO nudge for a RANKED row that publishes a price_source with
/// no basis — unranked rows are 淘汰列 that nobody compares, so they stay quiet.
pub(crate) fn missing_basis_info(r: &AccommodationBasisRow) -> Option<String> {
    if r.ranking.is_some()
        && r.price_basis.is_none()
        && r.price_source.as_deref().is_some_and(|s| !s.trim().is_empty())
    {
        Some(format!(
            "{dest}/{hotel} ({id}) has a price_source but no price_basis (incl=含稅 / excl=未稅) — \
             set via update-accommodation --id {id} --price-basis incl|excl",
            dest = r.destination,
            hotel = r.hotel,
            id = r.id
        ))
    } else {
        None
    }
}

/// Pure rule behind the mix WARN: within ONE destination's RANKED candidates,
/// both incl and excl bases present means the ranking compares apples to
/// oranges (Booking 含稅 vs Agoda 未稅). NULL/dirty bases do not count toward
/// the mix (dirty gets its own WARN; NULL is 未確認, tracked by the INFO).
pub(crate) fn basis_mix_issue(dest: &str, rows: &[AccommodationBasisRow]) -> Option<String> {
    let incl: Vec<&str> = rows
        .iter()
        .filter(|r| r.ranking.is_some() && r.price_basis.as_deref() == Some("incl"))
        .map(|r| r.id.as_str())
        .collect();
    let excl: Vec<&str> = rows
        .iter()
        .filter(|r| r.ranking.is_some() && r.price_basis.as_deref() == Some("excl"))
        .map(|r| r.id.as_str())
        .collect();
    if incl.is_empty() || excl.is_empty() {
        return None;
    }
    Some(format!(
        "{dest} 排序中的候選混用價格口徑：含稅 {n} 列（{incl}）、未稅 {m} 列（{excl}）— \
         排序比較不可信，請換算或補查同口徑價格",
        n = incl.len(),
        incl = incl.join(", "),
        m = excl.len(),
        excl = excl.join(", ")
    ))
}

#[cfg(test)]
mod price_basis_validate_tests {
    use super::*;

    fn row(dest: &str, id: &str, ranking: Option<i64>, source: Option<&str>, basis: Option<&str>) -> AccommodationBasisRow {
        AccommodationBasisRow {
            destination: dest.into(),
            id: id.into(),
            hotel: format!("H-{id}"),
            ranking,
            price_source: source.map(str::to_string),
            price_basis: basis.map(str::to_string),
        }
    }

    #[test]
    fn mix_warns_when_one_destination_ranks_both_bases() {
        let rows = vec![
            row("ne_coast", "a1", Some(1), Some("Booking.com"), Some("incl")),
            row("ne_coast", "a2", Some(2), Some("Agoda"), Some("excl")),
        ];
        let msg = basis_mix_issue("ne_coast", &rows).expect("mixed ranked bases → WARN");
        assert!(msg.contains("ne_coast") && msg.contains("含稅 1 列") && msg.contains("未稅 1 列"), "{msg}");
        assert!(msg.contains("a1") && msg.contains("a2"), "ids listed: {msg}");
        assert!(msg.contains("排序比較不可信"), "{msg}");
    }

    #[test]
    fn single_basis_no_mix() {
        let rows = vec![
            row("ne_coast", "a1", Some(1), Some("Booking.com"), Some("incl")),
            row("ne_coast", "a2", Some(2), Some("Booking.com"), Some("incl")),
        ];
        assert!(basis_mix_issue("ne_coast", &rows).is_none());
    }

    #[test]
    fn unranked_null_and_dirty_rows_do_not_count_toward_the_mix() {
        // Unranked excl next to ranked incl → not a comparison, no WARN.
        let unranked = vec![
            row("d", "a1", Some(1), None, Some("incl")),
            row("d", "a2", None, None, Some("excl")),
        ];
        assert!(basis_mix_issue("d", &unranked).is_none());
        // NULL basis is 未確認, not a side of the mix.
        let nulls = vec![
            row("d", "a1", Some(1), None, Some("incl")),
            row("d", "a2", Some(2), None, None),
        ];
        assert!(basis_mix_issue("d", &nulls).is_none());
        // Dirty value gets its own WARN, never the mix.
        let dirty = vec![
            row("d", "a1", Some(1), None, Some("incl")),
            row("d", "a2", Some(2), None, Some("weird")),
        ];
        assert!(basis_mix_issue("d", &dirty).is_none());
        assert!(invalid_basis_line(&dirty[1]).is_some());
    }

    #[test]
    fn destinations_do_not_leak_into_each_other() {
        // Same ids shape across two dests, each single-basis → no WARN per dest.
        let ne = vec![row("ne_coast", "a1", Some(1), None, Some("incl"))];
        let jf = vec![row("jiufen", "j1", Some(1), None, Some("excl"))];
        assert!(basis_mix_issue("ne_coast", &ne).is_none());
        assert!(basis_mix_issue("jiufen", &jf).is_none());
    }

    #[test]
    fn invalid_basis_names_id_and_value() {
        assert!(invalid_basis_line(&row("d", "x9", Some(1), Some("Booking.com"), Some("tax-free")))
            .unwrap()
            .contains("price_basis 'tax-free' is not incl or excl"));
        // Legal values and NULL pass.
        assert!(invalid_basis_line(&row("d", "x9", None, None, Some("incl"))).is_none());
        assert!(invalid_basis_line(&row("d", "x9", None, None, Some("excl"))).is_none());
        assert!(invalid_basis_line(&row("d", "x9", None, None, None)).is_none());
    }

    #[test]
    fn missing_basis_info_only_for_ranked_rows_with_a_source() {
        // Ranked + source + no basis → INFO naming the fix.
        let msg = missing_basis_info(&row("ne_coast", "a1", Some(1), Some("Booking.com"), None)).unwrap();
        assert!(msg.contains("update-accommodation --id a1 --price-basis"), "{msg}");
        // Unranked → quiet (淘汰列不比價).
        assert!(missing_basis_info(&row("d", "a2", None, Some("Booking.com"), None)).is_none());
        // Ranked but no source → nothing published, quiet.
        assert!(missing_basis_info(&row("d", "a3", Some(1), None, None)).is_none());
        // Ranked + source + basis already set → quiet.
        assert!(missing_basis_info(&row("d", "a4", Some(1), Some("Agoda"), Some("excl"))).is_none());
    }
}

/// [bookings] drift between the plans' normalized tables and `bookings_current`
/// — closes the blind spot where an activity sat at booking_status='booked'
/// while its bookings_current row said 'pending' for a month with every
/// validator green (self-review 2026-10-29). Runs for BOTH `validate data` and
/// `doctor` (always-run data-consistency block).
///
/// NOT best-effort, unlike the map helpers below: a check that cannot run
/// (connect/plan-list/compare failure) is an ERROR issue — a silent skip would
/// recreate the blind spot this exists to close. Drift and missing derived
/// rows are WARNINGS so historical unsynced plans don't block the pre-commit
/// hook; promote to error only after measuring real data. Only the DEFAULT
/// trip_id mapping (plan_id hyphens→underscores) is checked — custom
/// `--trip-id` syncs can't be attributed to a plan. Accommodation db-only
/// rows are excluded by `compare_bookings` (set-accommodation writes them
/// directly; they are not plan-derived).
async fn validate_booking_integrity(issues: &mut Vec<Issue>) {
    let conn = match db::connect_read().await {
        Ok(c) => c,
        Err(e) => {
            issues.push(Issue {
                category: "bookings".to_string(),
                severity: Severity::Error,
                message: format!("booking integrity check could not connect: {e}"),
                file: Some("turso:bookings_current".to_string()),
                line: None,
            });
            return;
        }
    };
    let mut rows = match conn
        .query(
            "SELECT plan_id FROM plans WHERE deleted_at IS NULL ORDER BY plan_id",
            (),
        )
        .await
    {
        Ok(r) => r,
        Err(e) => {
            issues.push(Issue {
                category: "bookings".to_string(),
                severity: Severity::Error,
                message: format!("booking integrity check could not list plans: {e}"),
                file: Some("turso:plans".to_string()),
                line: None,
            });
            return;
        }
    };
    let mut plan_ids: Vec<String> = Vec::new();
    // NOT best-effort iteration: a row-read error is the END-of-loop arm of
    // `while let Ok(Some(..))`, which would silently skip every remaining
    // plan and still report success (codex review 2026-10-29). Handle both
    // failure modes explicitly as errors.
    loop {
        match rows.next().await {
            Ok(Some(row)) => match row.get::<String>(0) {
                Ok(id) => plan_ids.push(id),
                Err(e) => {
                    issues.push(Issue {
                        category: "bookings".to_string(),
                        severity: Severity::Error,
                        message: format!(
                            "booking integrity check could not read plan id: {e}"
                        ),
                        file: Some("turso:plans".to_string()),
                        line: None,
                    });
                    return;
                }
            },
            Ok(None) => break,
            Err(e) => {
                issues.push(Issue {
                    category: "bookings".to_string(),
                    severity: Severity::Error,
                    message: format!(
                        "booking integrity check failed while listing plans: {e}"
                    ),
                    file: Some("turso:plans".to_string()),
                    line: None,
                });
                return;
            }
        }
    }

    for plan_id in plan_ids {
        let trip_id = plan_id.replace('-', "_");
        let report = match crate::booking_integrity::compare_bookings(&conn, &plan_id, &trip_id)
            .await
        {
            Ok(r) => r,
            Err(e) => {
                issues.push(Issue {
                    category: "bookings".to_string(),
                    severity: Severity::Error,
                    message: format!(
                        "booking integrity check failed for plan {plan_id}: {e}"
                    ),
                    file: Some(format!("plan:{plan_id}")),
                    line: None,
                });
                continue;
            }
        };
        for f in &report.mismatches {
            issues.push(Issue {
                category: "bookings".to_string(),
                severity: Severity::Warning,
                message: format!(
                    "{plan_id}: {}: {} ({}) [{}]",
                    f.booking_key, f.detail, f.title, f.category
                ),
                file: Some("turso:bookings_current".to_string()),
                line: None,
            });
        }
        for f in &report.plan_only {
            issues.push(Issue {
                category: "bookings".to_string(),
                severity: Severity::Warning,
                message: format!(
                    "{plan_id}: plan-only booking not in bookings_current: {} ({}: {}) — \
                     run: travel sync-bookings --plan-id {plan_id}",
                    f.booking_key, f.category, f.title
                ),
                file: Some("turso:bookings_current".to_string()),
                line: None,
            });
        }
        for f in &report.db_only {
            issues.push(Issue {
                category: "bookings".to_string(),
                severity: Severity::Warning,
                message: format!(
                    "{plan_id}: db-only booking not derivable from plan tables: {} ({}: {}) — \
                     stale row from an old itinerary? — \
                     run: travel sync-bookings --plan-id {plan_id}",
                    f.booking_key, f.category, f.title
                ),
                file: Some("turso:bookings_current".to_string()),
                line: None,
            });
        }
        for w in &report.extractor_warnings {
            issues.push(Issue {
                category: "bookings".to_string(),
                severity: Severity::Warning,
                message: format!("{plan_id}: extractor warning: {w}"),
                file: Some(format!("plan:{plan_id}")),
                line: None,
            });
        }
    }
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
                        "maps never snapshotted — run travel snapshot-maps --plan-id {plan_id}"
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
                        "itinerary changed since maps snapshotted ({snapshotted_at}) — maps STALE, re-run travel snapshot-maps"
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
            message: "Turso ota_sources has no rows. Run ./bin/travel db migrate.".to_string(),
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

    // Planning-flow docs consistency (T5 + the 2026-07-02 walk-back): the known-flights FAST-PATH is
    // the documented default (no "classify the trip" router — trips don't vary in kind), and Stage-2
    // purchase modes must match the flow_decision.rs MODES. One narrow block, not per-phrase
    // brittleness (Codex-advised minimal oracle).
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

/// Pure matcher for the planning-flow docs. Returns one message per missing required element.
/// Asserts the known-flights fast-path is documented as the default and the Stage-2 purchase modes
/// (`shop`/`ingest-known`/`defer`, matching flow_decision.rs MODES) are present with
/// validation-mandatory stated. The "trip-intake router" classification framing was REMOVED
/// 2026-07-02 (it was a purchase-option matrix mis-framed as trip classification; all trips are the
/// same known-flights shape), so this guard no longer requires it. Kept narrow + testable.
fn planning_flow_doc_violations(content: &str) -> Vec<String> {
    let mut out = Vec::new();
    if !content.contains("known-flights fast-path") {
        out.push(
            "CLAUDE.md must document the known-flights fast-path as the default planning path"
                .to_string(),
        );
    }
    // Stage-2 purchase modes must all appear + validation-mandatory stated (must match MODES).
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
        .query(
            "SELECT slug, currency FROM destination_config ORDER BY slug",
            (),
        )
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
    let mut jpy_slugs: Vec<String> = Vec::new();
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
        let currency: String = row.get(1).unwrap_or_default();
        slugs.push(slug.clone());
        // destination_areas is Japan Shaping machinery (seed covers only
        // Japan destinations). Domestic destinations have no area research
        // path — an empty destination_areas there is not a gap.
        if currency == "JPY" {
            jpy_slugs.push(slug);
        }
    }

    if slugs.is_empty() {
        issues.push(Issue {
            category: "destinations".to_string(),
            severity: Severity::Error,
            message: "Turso destination_config has no rows. Run ./bin/travel db migrate.".to_string(),
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

    for slug in &jpy_slugs {
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

    // Ungeocoded POIs: destination_pois rows missing lat/lon (map pins can't be
    // rendered on the dashboard until a real geocoder value is set via
    // `travel set-poi-coords`). Grouped by slug with a count — WARNING only
    // (never blocks `validate data`'s exit code).
    let mut ungeocoded_rows = match conn
        .query(
            "SELECT slug, COUNT(*) AS n FROM destination_pois \
             WHERE lat IS NULL OR lon IS NULL \
                OR TRIM(CAST(lat AS TEXT)) = '' OR TRIM(CAST(lon AS TEXT)) = '' \
             GROUP BY slug ORDER BY slug",
            (),
        )
        .await
    {
        Ok(r) => r,
        Err(err) => {
            issues.push(Issue {
                category: "destinations".to_string(),
                severity: Severity::Error,
                message: format!("failed to query destination_pois for ungeocoded rows: {err}"),
                file: Some("turso:destination_pois".to_string()),
                line: None,
            });
            return;
        }
    };
    while let Some(row) = ungeocoded_rows.next().await.ok().flatten() {
        let slug: String = row.get(0).unwrap_or_default();
        let n: i64 = row.get(1).unwrap_or(0);
        issues.push(Issue {
            category: "destinations".to_string(),
            severity: Severity::Warning,
            message: format!(
                "{slug}: {n} destination_pois row(s) missing lat/lon. Run ./bin/travel set-poi-coords <slug> <poi_id> <lat> <lon>."
            ),
            file: Some("turso:destination_pois".to_string()),
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
            message: "Turso holidays has no rows. Run ./bin/travel db migrate and ./bin/travel db fetch holidays <DGPA CSV URL>.".to_string(),
            file: Some("turso:holidays".to_string()),
            line: None,
        });
    }
}

// --- DB check: omiyage reference integrity ---
//
// Dedicated path (NOT the empty-table-is-error checklist below). Empty
// destination_omiyage_* tables are valid — only row-level invariants error.

async fn validate_omiyage(issues: &mut Vec<Issue>) {
    let conn = match db::connect_read().await {
        Ok(c) => c,
        Err(err) => {
            issues.push(Issue {
                category: "omiyage".to_string(),
                severity: Severity::Error,
                message: format!("failed to connect to Turso: {err}"),
                file: Some("turso:destination_omiyage_*".to_string()),
                line: None,
            });
            return;
        }
    };

    // 1. Orphan location: (slug,item_id) has no matching destination_omiyage_items row
    {
        let mut rows = match conn
            .query(
                "SELECT l.slug, l.item_id, l.poi_id \
                 FROM destination_omiyage_locations l \
                 LEFT JOIN destination_omiyage_items i \
                   ON i.slug = l.slug AND i.item_id = l.item_id \
                 WHERE i.item_id IS NULL \
                 ORDER BY l.slug, l.item_id, l.poi_id",
                (),
            )
            .await
        {
            Ok(r) => r,
            Err(err) => {
                issues.push(Issue {
                    category: "omiyage".to_string(),
                    severity: Severity::Error,
                    message: format!("failed to query omiyage orphan locations: {err}"),
                    file: Some("turso:destination_omiyage_locations".to_string()),
                    line: None,
                });
                return;
            }
        };
        while let Some(row) = rows.next().await.ok().flatten() {
            let slug: String = row.get(0).unwrap_or_default();
            let item_id: String = row.get(1).unwrap_or_default();
            let poi_id: String = row.get(2).unwrap_or_default();
            issues.push(Issue {
                category: "omiyage".to_string(),
                severity: Severity::Error,
                message: format!(
                    "{slug}: omiyage location item_id '{item_id}' poi_id '{poi_id}' has no parent item (orphan location)"
                ),
                file: Some("turso:destination_omiyage_locations".to_string()),
                line: None,
            });
        }
    }

    // 2. Orphan seller POI: (slug,poi_id) has no matching destination_pois row
    {
        let mut rows = match conn
            .query(
                "SELECT l.slug, l.item_id, l.poi_id \
                 FROM destination_omiyage_locations l \
                 LEFT JOIN destination_pois p \
                   ON p.slug = l.slug AND p.poi_id = l.poi_id \
                 WHERE p.poi_id IS NULL \
                 ORDER BY l.slug, l.item_id, l.poi_id",
                (),
            )
            .await
        {
            Ok(r) => r,
            Err(err) => {
                issues.push(Issue {
                    category: "omiyage".to_string(),
                    severity: Severity::Error,
                    message: format!("failed to query omiyage orphan seller POIs: {err}"),
                    file: Some("turso:destination_omiyage_locations".to_string()),
                    line: None,
                });
                return;
            }
        };
        while let Some(row) = rows.next().await.ok().flatten() {
            let slug: String = row.get(0).unwrap_or_default();
            let item_id: String = row.get(1).unwrap_or_default();
            let poi_id: String = row.get(2).unwrap_or_default();
            issues.push(Issue {
                category: "omiyage".to_string(),
                severity: Severity::Error,
                message: format!(
                    "{slug}: omiyage location item_id '{item_id}' poi_id '{poi_id}' has no matching destination_pois row (orphan seller POI)"
                ),
                file: Some("turso:destination_omiyage_locations".to_string()),
                line: None,
            });
        }
    }

    // 3. Item with zero locations (item with no seller)
    {
        let mut rows = match conn
            .query(
                "SELECT i.slug, i.item_id \
                 FROM destination_omiyage_items i \
                 LEFT JOIN destination_omiyage_locations l \
                   ON l.slug = i.slug AND l.item_id = i.item_id \
                 WHERE l.item_id IS NULL \
                 ORDER BY i.slug, i.item_id",
                (),
            )
            .await
        {
            Ok(r) => r,
            Err(err) => {
                issues.push(Issue {
                    category: "omiyage".to_string(),
                    severity: Severity::Error,
                    message: format!("failed to query omiyage items without location: {err}"),
                    file: Some("turso:destination_omiyage_items".to_string()),
                    line: None,
                });
                return;
            }
        };
        while let Some(row) = rows.next().await.ok().flatten() {
            let slug: String = row.get(0).unwrap_or_default();
            let item_id: String = row.get(1).unwrap_or_default();
            issues.push(Issue {
                category: "omiyage".to_string(),
                severity: Severity::Error,
                message: format!(
                    "{slug}: omiyage item '{item_id}' has no location (item without seller)"
                ),
                file: Some("turso:destination_omiyage_items".to_string()),
                line: None,
            });
        }
    }

    // 4. item.slug NOT IN destination_config
    {
        let mut rows = match conn
            .query(
                "SELECT i.slug, i.item_id \
                 FROM destination_omiyage_items i \
                 LEFT JOIN destination_config c ON c.slug = i.slug \
                 WHERE c.slug IS NULL \
                 ORDER BY i.slug, i.item_id",
                (),
            )
            .await
        {
            Ok(r) => r,
            Err(err) => {
                issues.push(Issue {
                    category: "omiyage".to_string(),
                    severity: Severity::Error,
                    message: format!("failed to query omiyage items vs destination_config: {err}"),
                    file: Some("turso:destination_omiyage_items".to_string()),
                    line: None,
                });
                return;
            }
        };
        while let Some(row) = rows.next().await.ok().flatten() {
            let slug: String = row.get(0).unwrap_or_default();
            let item_id: String = row.get(1).unwrap_or_default();
            issues.push(Issue {
                category: "omiyage".to_string(),
                severity: Severity::Error,
                message: format!(
                    "{slug}: omiyage item '{item_id}' slug not in destination_config"
                ),
                file: Some("turso:destination_omiyage_items".to_string()),
                line: None,
            });
        }
    }

    // 5. Item blank/NULL required fields (name / category / source_url / fetched_at / confidence)
    {
        let mut rows = match conn
            .query(
                "SELECT slug, item_id, \
                        CASE WHEN NULLIF(TRIM(COALESCE(name,'')),'') IS NULL THEN 1 ELSE 0 END, \
                        CASE WHEN NULLIF(TRIM(COALESCE(category,'')),'') IS NULL THEN 1 ELSE 0 END, \
                        CASE WHEN NULLIF(TRIM(COALESCE(source_url,'')),'') IS NULL THEN 1 ELSE 0 END, \
                        CASE WHEN NULLIF(TRIM(COALESCE(fetched_at,'')),'') IS NULL THEN 1 ELSE 0 END, \
                        CASE WHEN NULLIF(TRIM(COALESCE(confidence,'')),'') IS NULL THEN 1 ELSE 0 END \
                 FROM destination_omiyage_items \
                 WHERE NULLIF(TRIM(COALESCE(name,'')),'') IS NULL \
                    OR NULLIF(TRIM(COALESCE(category,'')),'') IS NULL \
                    OR NULLIF(TRIM(COALESCE(source_url,'')),'') IS NULL \
                    OR NULLIF(TRIM(COALESCE(fetched_at,'')),'') IS NULL \
                    OR NULLIF(TRIM(COALESCE(confidence,'')),'') IS NULL \
                 ORDER BY slug, item_id",
                (),
            )
            .await
        {
            Ok(r) => r,
            Err(err) => {
                issues.push(Issue {
                    category: "omiyage".to_string(),
                    severity: Severity::Error,
                    message: format!("failed to query omiyage item required fields: {err}"),
                    file: Some("turso:destination_omiyage_items".to_string()),
                    line: None,
                });
                return;
            }
        };
        while let Some(row) = rows.next().await.ok().flatten() {
            let slug: String = row.get(0).unwrap_or_default();
            let item_id: String = row.get(1).unwrap_or_default();
            let miss_name: i64 = row.get(2).unwrap_or(0);
            let miss_cat: i64 = row.get(3).unwrap_or(0);
            let miss_url: i64 = row.get(4).unwrap_or(0);
            let miss_fetched: i64 = row.get(5).unwrap_or(0);
            let miss_conf: i64 = row.get(6).unwrap_or(0);
            let mut fields: Vec<&str> = Vec::new();
            if miss_name != 0 {
                fields.push("name");
            }
            if miss_cat != 0 {
                fields.push("category");
            }
            if miss_url != 0 {
                fields.push("source_url");
            }
            if miss_fetched != 0 {
                fields.push("fetched_at");
            }
            if miss_conf != 0 {
                fields.push("confidence");
            }
            let field_list = fields.join("/");
            issues.push(Issue {
                category: "omiyage".to_string(),
                severity: Severity::Error,
                message: format!(
                    "{slug}: omiyage item '{item_id}' missing provenance/required field(s): {field_list}"
                ),
                file: Some("turso:destination_omiyage_items".to_string()),
                line: None,
            });
        }
    }

    // 6. Location blank/NULL required fields (source_url / fetched_at / confidence)
    {
        let mut rows = match conn
            .query(
                "SELECT slug, item_id, poi_id, \
                        CASE WHEN NULLIF(TRIM(COALESCE(source_url,'')),'') IS NULL THEN 1 ELSE 0 END, \
                        CASE WHEN NULLIF(TRIM(COALESCE(fetched_at,'')),'') IS NULL THEN 1 ELSE 0 END, \
                        CASE WHEN NULLIF(TRIM(COALESCE(confidence,'')),'') IS NULL THEN 1 ELSE 0 END \
                 FROM destination_omiyage_locations \
                 WHERE NULLIF(TRIM(COALESCE(source_url,'')),'') IS NULL \
                    OR NULLIF(TRIM(COALESCE(fetched_at,'')),'') IS NULL \
                    OR NULLIF(TRIM(COALESCE(confidence,'')),'') IS NULL \
                 ORDER BY slug, item_id, poi_id",
                (),
            )
            .await
        {
            Ok(r) => r,
            Err(err) => {
                issues.push(Issue {
                    category: "omiyage".to_string(),
                    severity: Severity::Error,
                    message: format!("failed to query omiyage location required fields: {err}"),
                    file: Some("turso:destination_omiyage_locations".to_string()),
                    line: None,
                });
                return;
            }
        };
        while let Some(row) = rows.next().await.ok().flatten() {
            let slug: String = row.get(0).unwrap_or_default();
            let item_id: String = row.get(1).unwrap_or_default();
            let poi_id: String = row.get(2).unwrap_or_default();
            let miss_url: i64 = row.get(3).unwrap_or(0);
            let miss_fetched: i64 = row.get(4).unwrap_or(0);
            let miss_conf: i64 = row.get(5).unwrap_or(0);
            let mut fields: Vec<&str> = Vec::new();
            if miss_url != 0 {
                fields.push("source_url");
            }
            if miss_fetched != 0 {
                fields.push("fetched_at");
            }
            if miss_conf != 0 {
                fields.push("confidence");
            }
            let field_list = fields.join("/");
            issues.push(Issue {
                category: "omiyage".to_string(),
                severity: Severity::Error,
                message: format!(
                    "{slug}: omiyage location item_id '{item_id}' poi_id '{poi_id}' missing provenance/required field(s): {field_list}"
                ),
                file: Some("turso:destination_omiyage_locations".to_string()),
                line: None,
            });
        }
    }

    // 7. confidence NOT IN ('verified','reviewed') on either table
    //    (non-blank values outside the allowed set; blank already covered above)
    {
        let mut rows = match conn
            .query(
                "SELECT slug, item_id, confidence \
                 FROM destination_omiyage_items \
                 WHERE NULLIF(TRIM(COALESCE(confidence,'')),'') IS NOT NULL \
                   AND TRIM(confidence) NOT IN ('verified','reviewed') \
                 ORDER BY slug, item_id",
                (),
            )
            .await
        {
            Ok(r) => r,
            Err(err) => {
                issues.push(Issue {
                    category: "omiyage".to_string(),
                    severity: Severity::Error,
                    message: format!("failed to query omiyage item confidence: {err}"),
                    file: Some("turso:destination_omiyage_items".to_string()),
                    line: None,
                });
                return;
            }
        };
        while let Some(row) = rows.next().await.ok().flatten() {
            let slug: String = row.get(0).unwrap_or_default();
            let item_id: String = row.get(1).unwrap_or_default();
            let confidence: String = row.get(2).unwrap_or_default();
            issues.push(Issue {
                category: "omiyage".to_string(),
                severity: Severity::Error,
                message: format!(
                    "{slug}: omiyage item '{item_id}' confidence '{confidence}' not in (verified, reviewed)"
                ),
                file: Some("turso:destination_omiyage_items".to_string()),
                line: None,
            });
        }
    }
    {
        let mut rows = match conn
            .query(
                "SELECT slug, item_id, poi_id, confidence \
                 FROM destination_omiyage_locations \
                 WHERE NULLIF(TRIM(COALESCE(confidence,'')),'') IS NOT NULL \
                   AND TRIM(confidence) NOT IN ('verified','reviewed') \
                 ORDER BY slug, item_id, poi_id",
                (),
            )
            .await
        {
            Ok(r) => r,
            Err(err) => {
                issues.push(Issue {
                    category: "omiyage".to_string(),
                    severity: Severity::Error,
                    message: format!("failed to query omiyage location confidence: {err}"),
                    file: Some("turso:destination_omiyage_locations".to_string()),
                    line: None,
                });
                return;
            }
        };
        while let Some(row) = rows.next().await.ok().flatten() {
            let slug: String = row.get(0).unwrap_or_default();
            let item_id: String = row.get(1).unwrap_or_default();
            let poi_id: String = row.get(2).unwrap_or_default();
            let confidence: String = row.get(3).unwrap_or_default();
            issues.push(Issue {
                category: "omiyage".to_string(),
                severity: Severity::Error,
                message: format!(
                    "{slug}: omiyage location item_id '{item_id}' poi_id '{poi_id}' confidence '{confidence}' not in (verified, reviewed)"
                ),
                file: Some("turso:destination_omiyage_locations".to_string()),
                line: None,
            });
        }
    }

    // 8. EMPTY tables → NO error (intentionally no COUNT(*)==0 check).
}

// --- DB check: clothing_tips (dashboard clothing-advice bands) ---

/// The dashboard has NO hardcoded clothing advice: an empty `clothing_tips`
/// table means every day card silently drops its "what to wear" line. WARN so
/// that gap is visible; fill via `travel set-clothing-tip`.
async fn validate_clothing_tips(issues: &mut Vec<Issue>) {
    let warn = |issues: &mut Vec<Issue>, message: String| {
        issues.push(Issue {
            category: "clothing-tips".to_string(),
            severity: Severity::Warning,
            message,
            file: Some("turso:clothing_tips".to_string()),
            line: None,
        });
    };
    let Ok(conn) = db::connect_read().await else {
        return;
    };
    match travel_db::repo::clothing_tips::count(&conn).await {
        Ok(0) => warn(
            issues,
            "clothing_tips is empty — the dashboard shows no clothing advice under the weather; \
             add bands via travel set-clothing-tip"
                .to_string(),
        ),
        // Bands exist: the weather overview's packing summary (要帶的衣物) counts
        // the bands' items — none anywhere means that summary never shows.
        Ok(_) => match travel_db::repo::clothing_tips::item_count(&conn).await {
            Ok(0) => issues.push(Issue {
                category: "clothing-tips".to_string(),
                severity: Severity::Info,
                message: "no clothing band has items — the weather overview shows no packing \
                          summary; add them via travel set-clothing-tip --item \"<zh>|<en>\""
                    .to_string(),
                file: Some("turso:clothing_tip_items".to_string()),
                line: None,
            }),
            Ok(_) => {}
            Err(e) if e.contains("no such table") => warn(
                issues,
                "clothing_tip_items table missing — run ./bin/travel db migrate (the dashboard \
                 fails to load plans until it exists)"
                    .to_string(),
            ),
            Err(e) => issues.push(Issue {
                category: "clothing-tips".to_string(),
                severity: Severity::Error,
                message: format!("failed to query clothing_tip_items: {e}"),
                file: Some("turso:clothing_tip_items".to_string()),
                line: None,
            }),
        },
        Err(e) if e.contains("no such table") => warn(
            issues,
            "clothing_tips table missing — run ./bin/travel db migrate, then add bands via travel set-clothing-tip"
                .to_string(),
        ),
        Err(e) => issues.push(Issue {
            category: "clothing-tips".to_string(),
            severity: Severity::Error,
            message: format!("failed to query clothing_tips: {e}"),
            file: Some("turso:clothing_tips".to_string()),
            line: None,
        }),
    }
}

// --- DB check: domestic_accommodations (Taiwan domestic stay reference data) ---

/// Every domestic_accommodations row should carry an image_url (WARN — the
/// dashboard renders a placeholder otherwise) and a booking_url (INFO — advisory
/// only; a stay can legitimately be phone/walk-in booked). Both are fixable via
/// `travel update-accommodation --id <id> [--image-url u] [--booking-url u]`.
/// A thin candidate gallery (< GALLERY_MIN extra photos in
/// domestic_accommodation_images) is an INFO — one hero photo per candidate reads
/// as too little to choose from; fix via `travel add-accommodation-image`.
async fn validate_domestic_accommodations(issues: &mut Vec<Issue>) {
    let Ok(conn) = db::connect_read().await else {
        return;
    };
    let mut rows = match conn
        .query(
            "SELECT id, destination, hotel_name, image_url, booking_url, price_source, price_checked_at, \
             CAST(julianday('now') - julianday(price_checked_at) AS INTEGER) AS price_age_days, \
             ranking, notes, price_basis \
             FROM domestic_accommodations ORDER BY destination, id",
            (),
        )
        .await
    {
        Ok(r) => r,
        Err(err) => {
            // Table may not exist on a pre-migrate DB — not a data error.
            if err.to_string().contains("no such table") {
                return;
            }
            if err.to_string().contains("no such column") {
                // price_basis is new (2026-11-19); a pre-migrate DB has no column.
                issues.push(Issue {
                    category: "domestic-accommodations".to_string(),
                    severity: Severity::Error,
                    message: format!(
                        "cannot check domestic_accommodations price basis: column missing — \
                         run ./bin/travel db migrate ({err})"
                    ),
                    file: Some("turso:domestic_accommodations".to_string()),
                    line: None,
                });
                return;
            }
            issues.push(Issue {
                category: "domestic-accommodations".to_string(),
                severity: Severity::Error,
                message: format!("failed to query domestic_accommodations: {err}"),
                file: Some("turso:domestic_accommodations".to_string()),
                line: None,
            });
            return;
        }
    };

    let mut stays: Vec<(String, String, String)> = Vec::new();
    let mut basis_rows: Vec<AccommodationBasisRow> = Vec::new();
    while let Ok(Some(row)) = rows.next().await {
        let id: String = row.get(0).unwrap_or_default();
        let dest: String = row.get(1).unwrap_or_default();
        let hotel: String = row.get(2).unwrap_or_default();
        let image_url: Option<String> = row.get(3).ok();
        let booking_url: Option<String> = row.get(4).ok();

        if image_url.as_deref().is_none_or(|s| s.trim().is_empty()) {
            issues.push(Issue {
                category: "domestic-accommodations".to_string(),
                severity: Severity::Warning,
                message: format!(
                    "{dest}/{hotel} ({id}) missing image_url — dashboard shows a placeholder; set via update-accommodation --id {id} --image-url <url>"
                ),
                file: Some("turso:domestic_accommodations".to_string()),
                line: None,
            });
        }
        if booking_url.as_deref().is_none_or(|s| s.trim().is_empty()) {
            issues.push(Issue {
                category: "domestic-accommodations".to_string(),
                severity: Severity::Info,
                message: format!(
                    "{dest}/{hotel} ({id}) has no booking_url — add the Agoda/booking link via update-accommodation --id {id} --booking-url <url>"
                ),
                file: Some("turso:domestic_accommodations".to_string()),
                line: None,
            });
        }
        // 比較內容 — the dashboard's comparison section renders from these. A candidate
        // without notes is a bare price grid the reader cannot decide from, so it is a
        // WARN (this is the standing reminder for the agent writing candidates);
        // ranking (推薦順位) missing is an INFO — a shortlist can be fully written
        // before the final order is picked.
        let ranking: Option<i64> = row.get(8).ok();
        let notes: Option<String> = row.get(9).ok();
        if notes.as_deref().is_none_or(|s| s.trim().is_empty()) {
            issues.push(Issue {
                category: "domestic-accommodations".to_string(),
                severity: Severity::Warning,
                message: format!(
                    "{dest}/{hotel} ({id}) has no comparison notes (優劣比較與推薦理由) — the dashboard comparison section renders from notes; write them via update-accommodation --id {id} --notes \"<優劣與推薦理由>\""
                ),
                file: Some("turso:domestic_accommodations".to_string()),
                line: None,
            });
        }
        if ranking.is_none() {
            issues.push(Issue {
                category: "domestic-accommodations".to_string(),
                severity: Severity::Info,
                message: format!(
                    "{dest}/{hotel} ({id}) has no ranking (推薦順位, 1=首選) — set via update-accommodation --id {id} --rank <n>"
                ),
                file: Some("turso:domestic_accommodations".to_string()),
                line: None,
            });
        }
        // A published rate goes stale. WARN once it is older than PRICE_STALE_DAYS,
        // and INFO when a price was recorded with no read date at all — the
        // dashboard renders that date, so a missing one publishes an undated number.
        let price_source: Option<String> = row.get(5).ok();
        let price_checked: Option<String> = row.get(6).ok();
        let age_days: Option<i64> = row.get(7).ok();
        // Price-basis lints (spec §3, 2026-11-19): the column has no CHECK, so
        // dirty values get a WARN; a ranked row publishing a source with no
        // basis gets an INFO nudge (unranked rows are 淘汰列, not compared).
        let price_basis: Option<String> = row.get(10).ok();
        let basis_row = AccommodationBasisRow {
            destination: dest.clone(),
            id: id.clone(),
            hotel: hotel.clone(),
            ranking,
            price_source: price_source.clone(),
            price_basis,
        };
        if let Some(message) = invalid_basis_line(&basis_row) {
            issues.push(Issue {
                category: "domestic-accommodations".to_string(),
                severity: Severity::Warning,
                message,
                file: Some("turso:domestic_accommodations".to_string()),
                line: None,
            });
        }
        if let Some(message) = missing_basis_info(&basis_row) {
            issues.push(Issue {
                category: "domestic-accommodations".to_string(),
                severity: Severity::Info,
                message,
                file: Some("turso:domestic_accommodations".to_string()),
                line: None,
            });
        }
        basis_rows.push(basis_row);
        if price_source.as_deref().is_some_and(|s| !s.trim().is_empty()) {
            match (price_checked.as_deref(), age_days) {
                (Some(c), Some(age)) if !c.trim().is_empty() && age > PRICE_STALE_DAYS => {
                    issues.push(Issue {
                        category: "domestic-accommodations".to_string(),
                        severity: Severity::Warning,
                        message: format!(
                            "{dest}/{hotel} ({id}) price was last checked {c} ({age} days ago) — the dashboard publishes that date; re-check and run update-accommodation --id {id} --price-source <name>"
                        ),
                        file: Some("turso:domestic_accommodations".to_string()),
                        line: None,
                    });
                }
                (None, _) | (Some(""), _) => {
                    issues.push(Issue {
                        category: "domestic-accommodations".to_string(),
                        severity: Severity::Info,
                        message: format!(
                            "{dest}/{hotel} ({id}) has a price_source but no price_checked_at — set it via update-accommodation --id {id} --price-source <name> so the published rate carries its read date"
                        ),
                        file: Some("turso:domestic_accommodations".to_string()),
                        line: None,
                    });
                }
                _ => {}
            }
        }
        stays.push((id, dest, hotel));
    }

    // Basis mix: one WARN per destination whose RANKED candidates carry BOTH
    // incl and excl — the ranking then compares 含稅 vs 未稅 prices and cannot
    // be trusted (the exact trap this column exists to catch).
    let mut dest_order: Vec<String> = Vec::new();
    for r in &basis_rows {
        if !dest_order.contains(&r.destination) {
            dest_order.push(r.destination.clone());
        }
    }
    for dest in &dest_order {
        let group: Vec<AccommodationBasisRow> =
            basis_rows.iter().filter(|r| r.destination == *dest).cloned().collect();
        if let Some(message) = basis_mix_issue(dest, &group) {
            issues.push(Issue {
                category: "domestic-accommodations".to_string(),
                severity: Severity::Warning,
                message,
                file: Some("turso:domestic_accommodations".to_string()),
                line: None,
            });
        }
    }

    // Gallery depth: one hero image per candidate is too little to choose from.
    let mut counts: Vec<(String, i64)> = Vec::new();
    match conn
        .query(
            "SELECT accommodation_id, COUNT(*) FROM domestic_accommodation_images GROUP BY accommodation_id",
            (),
        )
        .await
    {
        Ok(mut g) => {
            while let Ok(Some(row)) = g.next().await {
                counts.push((row.get(0).unwrap_or_default(), row.get(1).unwrap_or(0)));
            }
        }
        // Pre-migrate DB without the child table — nothing to check.
        Err(err) if err.to_string().contains("no such table") => return,
        Err(err) => {
            issues.push(Issue {
                category: "domestic-accommodations".to_string(),
                severity: Severity::Error,
                message: format!("failed to query domestic_accommodation_images: {err}"),
                file: Some("turso:domestic_accommodation_images".to_string()),
                line: None,
            });
            return;
        }
    }
    for (id, dest, hotel) in &stays {
        let n = counts
            .iter()
            .find(|(aid, _)| aid == id)
            .map_or(0, |(_, n)| *n);
        if n < GALLERY_MIN {
            issues.push(Issue {
                category: "domestic-accommodations".to_string(),
                severity: Severity::Info,
                message: format!(
                    "{dest}/{hotel} ({id}) has only {n} gallery photo(s) — aim for {GALLERY_MIN}+ room/view shots via add-accommodation-image --id {id} --url <url> --label <text>"
                ),
                file: Some("turso:domestic_accommodation_images".to_string()),
                line: None,
            });
        }
    }
}

/// Minimum extra gallery photos a candidate should carry beyond its hero image.
const GALLERY_MIN: i64 = 3;

/// A published room rate older than this is flagged — OTA rates and "rooms left"
/// move week to week, and the dashboard shows the date it was read.
const PRICE_STALE_DAYS: i64 = 21;

/// WARN when a plan whose P4 (process_4_accommodation) is booked/confirmed
/// still carries an undecided-stay placeholder (住宿待定/住宿未定) in a route
/// endpoint or activity title (spec §2 A3, 2026-11-19). While the stay is
/// UNDECIDED the placeholder is the convention (routing.md 國內行程) — the lint
/// only fires once the stay is decided, when `set-accommodation` adds the hotel
/// pin and the placeholder text goes stale. One SELECT over the three text
/// locations, joined to the plan+destination's P4 row; the rule itself lives in
/// [`stay_placeholder_lines`].
async fn validate_stay_placeholders(issues: &mut Vec<Issue>) {
    let conn = match db::connect_read().await {
        Ok(conn) => conn,
        Err(err) => {
            // A connection failure is never a pass: this check did not run.
            issues.push(Issue {
                category: "stay-placeholder".to_string(),
                severity: Severity::Error,
                message: format!("stay placeholder check could not connect: {err}"),
                file: None,
                line: None,
            });
            return;
        }
    };
    let scanned = match travel_db::repo::stay_placeholders::scan(&conn).await {
        Ok(rows) => rows
            .into_iter()
            .map(|r| StayPlaceholderRow {
                src: match r.src.as_str() {
                    "day_route_segments.from_place" => "day_route_segments.from_place",
                    "day_route_segments.to_place" => "day_route_segments.to_place",
                    _ => "activities.title",
                },
                plan_id: r.plan_id,
                day: r.day,
                session: r.session,
                text: r.text,
                p4_status: r.p4_status,
            })
            .collect::<Vec<_>>(),
        Err(err) => {
            if let Some(message) = stay_scan_failure_message(&err) {
                issues.push(Issue {
                    category: "stay-placeholder".to_string(),
                    severity: Severity::Error,
                    message,
                    file: None,
                    line: None,
                });
            }
            return;
        }
    };
    for message in stay_placeholder_lines(&scanned) {
        issues.push(Issue {
            category: "stay-placeholder".to_string(),
            severity: Severity::Warning,
            message,
            file: None,
            line: None,
        });
    }
}

// ── activity geo / route via / requested_by lints (spec 2026-11-20 §5, §7.3) ──

/// The requester keyword an activity title carried before `requested_by` existed.
/// Keyword list is CODE (matched against data), like the stay markers.
const REQUESTER_TITLE_KEYWORD: &str = "指定";
/// `requested_by` length cap in characters (mirrors the CLI's 1–16 rule).
const REQUESTED_BY_MAX_CHARS: usize = 16;

fn hub_km(hub: Option<(f64, f64)>, lat: f64, lon: f64) -> Option<f64> {
    hub.map(|h| crate::snapshot_maps::haversine_m(h, (lat, lon)) / 1000.0)
}

fn act_loc(r: &travel_db::repo::geo_lint::ActivityGeoRow) -> String {
    format!("[plan {} day {} {}] 「{}」", r.plan_id, r.day, r.session, r.title)
}

/// Pure rule: activity pin sanity. ERROR on a half-set pair or an invalid
/// coordinate (reuses `checks::validate_coords`), INFO beyond the hub gate
/// (INFO — Japan trips legitimately carry Taiwan return-leg stops), INFO for two same-day activities pinned to the
/// same point at 5 decimals.
pub(crate) fn activity_geo_lines(
    rows: &[travel_db::repo::geo_lint::ActivityGeoRow],
) -> Vec<(Severity, String)> {
    let mut out = Vec::new();
    let mut by_point: std::collections::BTreeMap<(String, String, i64, String), Vec<String>> =
        std::collections::BTreeMap::new();
    for r in rows {
        match (r.lat, r.lon) {
            (None, None) => {}
            (Some(_), None) | (None, Some(_)) => out.push((
                Severity::Error,
                format!(
                    "activity has only one of lat/lon set (lat={:?} lon={:?}) — set both with set-activity-geocode, or clear both {}",
                    r.lat, r.lon, act_loc(r)
                ),
            )),
            (Some(lat), Some(lon)) => match crate::checks::validate_coords(lat, lon) {
                Err(e) => out.push((
                    Severity::Error,
                    format!("activity coordinates invalid ({e}) {}", act_loc(r)),
                )),
                Ok(_) => {
                    if let Some(km) = hub_km(r.hub, lat, lon) {
                        if km > crate::snapshot_maps::MAX_HUB_DISTANCE_KM {
                            out.push((
                                Severity::Info,
                                format!(
                                    "activity pin {lat},{lon} is far from the destination hub ({km:.0} km from {}; gate {:.0} km) — check the pin {}",
                                    r.destination,
                                    crate::snapshot_maps::MAX_HUB_DISTANCE_KM,
                                    act_loc(r)
                                ),
                            ));
                        }
                    }
                    by_point
                        .entry((r.plan_id.clone(), r.destination.clone(), r.day, format!("{lat:.5},{lon:.5}")))
                        .or_default()
                        .push(r.title.clone());
                }
            },
        }
    }
    for ((plan, _dest, day, pt), titles) in by_point {
        if titles.len() > 1 {
            out.push((
                Severity::Info,
                format!(
                    "{} activities share the same pin {pt} [plan {plan} day {day}]: {}",
                    titles.len(),
                    titles.iter().map(|t| format!("「{t}」")).collect::<Vec<_>>().join("、")
                ),
            ));
        }
    }
    out
}

/// Pure rule: `requested_by` hygiene. WARN over-long / multi-line (dirty
/// `db exec` data); INFO a title still carrying 「指定」 with no `requested_by`.
pub(crate) fn requested_by_lines(
    rows: &[travel_db::repo::geo_lint::ActivityGeoRow],
) -> Vec<(Severity, String)> {
    let mut out = Vec::new();
    for r in rows {
        match &r.requested_by {
            Some(v) => {
                if v.chars().count() > REQUESTED_BY_MAX_CHARS || v.contains('\n') || v.contains('\r') {
                    out.push((
                        Severity::Warning,
                        format!(
                            "requested_by is longer than {REQUESTED_BY_MAX_CHARS} characters or contains a newline — fix with update-activity --requested-by <name> {}",
                            act_loc(r)
                        ),
                    ));
                }
            }
            None => {
                // 指定席 = reserved seat (train ticket), not a requester.
                if r.title.replace("指定席", "").contains(REQUESTER_TITLE_KEYWORD) {
                    out.push((
                        Severity::Info,
                        format!(
                            "title still carries 「{REQUESTER_TITLE_KEYWORD}」 but requested_by is empty — move it with update-activity --requested-by <name> {}",
                            act_loc(r)
                        ),
                    ));
                }
            }
        }
    }
    out
}

/// Pure rule: stored via points. WARN a point whose segment row is gone
/// (orphan, one line per segment) and a point beyond the hub gate.
pub(crate) fn via_lines(rows: &[travel_db::repo::geo_lint::ViaGeoRow]) -> Vec<(Severity, String)> {
    let mut out = Vec::new();
    let mut orphans: std::collections::BTreeMap<(String, i64, i64), usize> = std::collections::BTreeMap::new();
    for r in rows {
        if !r.has_segment {
            *orphans.entry((r.plan_id.clone(), r.day, r.sort_order)).or_default() += 1;
        }
        if let Some(km) = hub_km(r.hub, r.lat, r.lon) {
            if km > crate::snapshot_maps::MAX_HUB_DISTANCE_KM {
                out.push((
                    Severity::Info,
                    format!(
                        "via point #{} {},{} is far from the destination hub ({km:.0} km from {}; gate {:.0} km) — check the pin [plan {} day {} segment {}]",
                        r.via_order, r.lat, r.lon, r.destination,
                        crate::snapshot_maps::MAX_HUB_DISTANCE_KM, r.plan_id, r.day, r.sort_order
                    ),
                ));
            }
        }
    }
    for ((plan, day, so), n) in orphans {
        out.push((
            Severity::Warning,
            format!(
                "{n} via point(s) have no matching day_route_segments row (orphan) [plan {plan} day {day} segment {so}] — clear with a segment rewrite or remove the rows"
            ),
        ));
    }
    out
}

async fn validate_activity_geo(issues: &mut Vec<Issue>) {
    let fail = |issues: &mut Vec<Issue>, cat: &str, msg: String| {
        issues.push(Issue {
            category: cat.to_string(),
            severity: Severity::Error,
            message: msg,
            file: None,
            line: None,
        });
    };
    let conn = match db::connect_read().await {
        Ok(c) => c,
        Err(err) => return fail(issues, "activity-geo", format!("activity geo check could not connect: {err}")),
    };
    let push = |issues: &mut Vec<Issue>, cat: &str, lines: Vec<(Severity, String)>| {
        for (severity, message) in lines {
            issues.push(Issue { category: cat.to_string(), severity, message, file: None, line: None });
        }
    };
    // A pre-migrate DB (columns/table missing) is reported by the other checks.
    match travel_db::repo::geo_lint::scan_activities(&conn, REQUESTER_TITLE_KEYWORD).await {
        Ok(rows) => {
            push(issues, "activity-geo", activity_geo_lines(&rows));
            push(issues, "requested-by", requested_by_lines(&rows));
        }
        Err(e) if e.contains("no such column") || e.contains("no such table") => {}
        Err(e) => fail(issues, "activity-geo", e),
    }
    match travel_db::repo::geo_lint::scan_via(&conn).await {
        Ok(rows) => push(issues, "route-via", via_lines(&rows)),
        Err(e) if e.contains("no such table") => {}
        Err(e) => fail(issues, "route-via", e),
    }
}

#[cfg(test)]
mod geo_lint_tests {
    use super::*;
    use travel_db::repo::geo_lint::{ActivityGeoRow, ViaGeoRow};

    const HUB: Option<(f64, f64)> = Some((24.86, 121.82)); // 頭城-ish

    fn act(title: &str, lat: Option<f64>, lon: Option<f64>, rb: Option<&str>) -> ActivityGeoRow {
        ActivityGeoRow {
            plan_id: "p".into(),
            destination: "d".into(),
            day: 1,
            session: "morning".into(),
            title: title.into(),
            lat,
            lon,
            requested_by: rb.map(|s| s.to_string()),
            hub: HUB,
        }
    }
    fn via(has_segment: bool, lat: f64, lon: f64) -> ViaGeoRow {
        ViaGeoRow {
            plan_id: "p".into(),
            destination: "d".into(),
            day: 2,
            sort_order: 1,
            via_order: 0,
            lat,
            lon,
            has_segment,
            hub: HUB,
        }
    }
    fn sev(v: &[(Severity, String)], s: Severity) -> usize {
        v.iter().filter(|(x, _)| *x == s).count()
    }

    #[test]
    fn half_set_pair_is_error() {
        let v = activity_geo_lines(&[act("a", Some(24.8), None, None), act("b", None, Some(121.8), None)]);
        assert_eq!(sev(&v, Severity::Error), 2, "{v:?}");
        assert!(activity_geo_lines(&[act("a", None, None, None)]).is_empty());
    }

    #[test]
    fn invalid_coords_are_errors_incl_null_island() {
        let v = activity_geo_lines(&[act("a", Some(95.0), Some(121.8), None), act("b", Some(0.0), Some(0.0), None)]);
        assert_eq!(sev(&v, Severity::Error), 2, "{v:?}");
        assert!(v[1].1.contains("null island"), "{v:?}");
    }

    #[test]
    fn far_from_hub_warns_and_swapped_pair_is_caught() {
        // swapped lat/lon of a Taiwan point lands in the Indian Ocean / far away
        let v = activity_geo_lines(&[act("swap", Some(121.8), Some(24.8), None)]);
        assert_eq!(sev(&v, Severity::Error), 1, "lat 121 is out of range: {v:?}");
        let v = activity_geo_lines(&[act("far", Some(35.0), Some(139.7), None)]);
        assert_eq!((sev(&v, Severity::Info), v.len()), (1, 1), "{v:?}");
        assert!(v[0].1.contains("far from the destination hub") && !v[0].1.contains("swapped"), "{v:?}");
        // inside the gate: silent; no hub: silent
        assert!(activity_geo_lines(&[act("near", Some(24.9), Some(121.8), None)]).is_empty());
        let mut r = act("nohub", Some(35.0), Some(139.7), None);
        r.hub = None;
        assert!(activity_geo_lines(&[r]).is_empty());
    }

    #[test]
    fn same_day_identical_coords_is_info_but_other_day_is_not() {
        let a = act("a", Some(24.90001), Some(121.80001), None);
        let b = act("b", Some(24.90001), Some(121.80001), None);
        let v = activity_geo_lines(&[a.clone(), b.clone()]);
        assert_eq!((sev(&v, Severity::Info), v.len()), (1, 1), "{v:?}");
        assert!(v[0].1.contains("share the same pin") && !v[0].1.contains("mis-pinned"), "{v:?}");
        // 0.0001 deg apart (~11 m) is a different 5-decimal point.
        let near = act("n", Some(24.90011), Some(121.80001), None);
        assert!(activity_geo_lines(&[a.clone(), near]).is_empty());
        let mut c = b;
        c.day = 2;
        assert!(activity_geo_lines(&[a, c]).is_empty());
    }

    #[test]
    fn requested_by_length_and_newline_warn_cjk_counts_chars() {
        let ok16 = "指".repeat(16);
        let bad17 = "指".repeat(17);
        assert!(requested_by_lines(&[act("a", None, None, Some(&ok16))]).is_empty());
        let v = requested_by_lines(&[act("a", None, None, Some(&bad17)), act("b", None, None, Some("x\ny"))]);
        assert_eq!(sev(&v, Severity::Warning), 2, "{v:?}");
    }

    #[test]
    fn title_keyword_without_requested_by_is_info_only_when_null() {
        let v = requested_by_lines(&[act("Ping 指定咖啡", None, None, None)]);
        assert_eq!(sev(&v, Severity::Info), 1, "{v:?}");
        assert!(v[0].1.contains("update-activity --requested-by"), "{v:?}");
        assert!(requested_by_lines(&[act("Ping 指定咖啡", None, None, Some("Ping"))]).is_empty());
        assert!(requested_by_lines(&[act("咖啡", None, None, None)]).is_empty());
        assert!(requested_by_lines(&[act("HARUKA指定席", None, None, None)]).is_empty());
    }

    #[test]
    fn via_orphan_and_far_points_warn() {
        let v = via_lines(&[via(false, 24.9, 121.8), via(false, 24.91, 121.81), via(true, 24.9, 121.8)]);
        assert_eq!((sev(&v, Severity::Warning), v.len()), (1, 1), "one line per orphan segment: {v:?}");
        assert!(v[0].1.contains("2 via point(s)") && v[0].1.contains("orphan"), "{v:?}");
        let v = via_lines(&[via(true, 35.0, 139.7)]);
        assert_eq!((sev(&v, Severity::Info), v.len()), (1, 1), "{v:?}");
        assert!(!v[0].1.contains("swapped"), "{v:?}");
        assert!(via_lines(&[via(true, 24.9, 121.8)]).is_empty());
    }
}

/// Pure rule for a failed A3 scan: ONLY an explicitly recognised missing-table
/// error (a pre-migrate DB — the other checks report it) may skip silently
/// (`None`); anything else (transient Turso error, bad row) is a validation
/// ERROR so a truncated scan can never read as "no stale placeholders".
pub(crate) fn stay_scan_failure_message(err: &str) -> Option<String> {
    if err.contains("no such table") {
        return None;
    }
    Some(format!("stay placeholder check could not complete: {err}"))
}

#[cfg(test)]
mod stay_scan_failure_tests {
    use super::stay_scan_failure_message;

    #[test]
    fn only_a_missing_table_skips_silently() {
        assert!(stay_scan_failure_message("stay placeholder scan failed: SQL error: no such table: activities").is_none());
    }

    #[test]
    fn any_other_failure_is_a_validation_error() {
        for e in [
            "stay placeholder scan failed: stream expired",
            "stay placeholder scan row failed: connection reset",
            "stay placeholder scan failed: no such column: x",
        ] {
            let m = stay_scan_failure_message(e).expect("must surface");
            assert!(m.contains("could not complete") && m.contains(e), "{m}");
        }
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
    // Skills may be listed in CLAUDE.md OR docs/reference/routing.md — the
    // Skill Decision Tree + Available Skills table moved to routing.md in the
    // 2026-09-28 rules-only CLAUDE.md restructure.
    let docs = ["CLAUDE.md", "docs/reference/routing.md"];
    let contents: Vec<(&str, String)> = docs
        .iter()
        .filter_map(|d| read_file(d).map(|c| (*d, c)))
        .collect();
    if contents.is_empty() {
        return;
    }
    let skill_path_re = match regex::Regex::new(r"`(src/skills/[^`]+/SKILL\.md)`") {
        Ok(r) => r,
        Err(_) => return,
    };
    let mut checked: HashSet<String> = HashSet::new();
    for (doc, content) in &contents {
        for m in skill_path_re.find_iter(content) {
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
                    file: Some(doc.to_string()),
                    line: None,
                });
            }
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
                    message: format!(
                        "Skill exists but not listed in CLAUDE.md or docs/reference/routing.md: {skill_md}"
                    ),
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
// Publish-readiness gate (`travel validate publish`) — READ-ONLY, no audit triad.
// ============================================================================

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum PublishSeverity {
    Block,
    Warn,
    Info,
}

pub(crate) struct PublishIssue {
    pub(crate) category: String,
    pub(crate) severity: PublishSeverity,
    pub(crate) message: String,
}

impl PublishIssue {
    pub(crate) fn is_block(&self) -> bool {
        self.severity == PublishSeverity::Block
    }
}

/// Run publish-readiness checks for one plan + destination. Exit 1 on any BLOCK.
async fn run_publish(plan_id: &str, dest_opt: Option<&str>) -> Result<(), String> {
    let conn = db::connect_read().await?;
    let destination =
        crate::cascade::common::resolve_active_destination(&conn, plan_id, dest_opt).await?;

    println!("Running publish-readiness validation for {plan_id} ({destination})...\n");

    let issues = collect_publish_issues(&conn, plan_id, &destination).await?;

    emit_publish_report(&issues);

    let blockers = issues
        .iter()
        .filter(|i| i.severity == PublishSeverity::Block)
        .count();
    if blockers > 0 {
        std::process::exit(1);
    }
    Ok(())
}

/// The publish-readiness checks for one plan + (already resolved) destination,
/// without printing. `validate publish` prints them; `set-process-status p5
/// confirmed` refuses on any Block. A past plan keeps its `is_past` handling
/// (no ZH/weather/content-depth checks).
pub(crate) async fn collect_publish_issues(
    conn: &libsql::Connection,
    plan_id: &str,
    destination: &str,
) -> Result<Vec<PublishIssue>, String> {
    let destination = destination.to_string();

    let today = crate::plans::today_iso_pub();
    let lifecycle = read_lifecycle(conn, plan_id, &destination).await?;
    let is_past = lifecycle.as_deref() == Some("past");
    let check_zh_weather = !is_past;

    let mut issues: Vec<PublishIssue> = Vec::new();

    // P5 days exist (BLOCK)
    let day_count = count_days(&conn, plan_id, &destination).await?;
    if day_count == 0 {
        issues.push(PublishIssue {
            category: "p5-days".to_string(),
            severity: PublishSeverity::Block,
            message: "no itinerary days found — run scaffold-itinerary first".to_string(),
        });
    }

    // Itinerary errors (BLOCK) — reuse validate-itinerary error path
    if day_count > 0 {
        let errors =
            crate::validate_itinerary::error_messages(conn, plan_id, &destination).await?;
        for msg in errors {
            issues.push(PublishIssue {
                category: "itinerary".to_string(),
                severity: PublishSeverity::Block,
                message: msg,
            });
        }
    }

    // Day-variant invariants (BLOCK)
    for msg in variant_violations(&conn, Some((plan_id, &destination))).await? {
        issues.push(PublishIssue {
            category: "variant".to_string(),
            severity: PublishSeverity::Block,
            message: msg,
        });
    }

    // ZH content (BLOCK) — upcoming/active only; empty sessions skipped via SQL
    if check_zh_weather {
        for day in missing_day_zh(&conn, plan_id, &destination).await? {
            issues.push(PublishIssue {
                category: "zh-content".to_string(),
                severity: PublishSeverity::Block,
                message: format!("day {day} missing theme_zh"),
            });
        }
        for (day, session) in missing_session_zh(&conn, plan_id, &destination).await? {
            issues.push(PublishIssue {
                category: "zh-content".to_string(),
                severity: PublishSeverity::Block,
                message: format!("day {day} {session} missing focus_zh/transit_notes_zh"),
            });
        }
    }

    // Map path (BLOCK) — POI coords OR day_route_segments
    if !has_map_path(&conn, plan_id, &destination).await? {
        issues.push(PublishIssue {
            category: "map-path".to_string(),
            severity: PublishSeverity::Block,
            message: "no map path — need POI coordinates on activities or day_route_segments"
                .to_string(),
        });
    }

    for day in map_coverage_gaps(&conn, plan_id, &destination).await? {
        issues.push(PublishIssue {
            category: "map-coverage".to_string(),
            severity: PublishSeverity::Warn,
            message: format!(
                "day {day} has no geocoded stops and no route segments - its dashboard map will render empty. Link a real POI: set-activity-poi {day} <session> <poi_id> (or set-activity-poi --auto)."
            ),
        });
    }

    // Maps freshness (WARN) — reuse check_maps_fresh::evaluate
    match crate::check_maps_fresh::evaluate(&conn, plan_id).await {
        Ok(crate::check_maps_fresh::Status::NeverSnapshotted) => {
            issues.push(PublishIssue {
                category: "maps-fresh".to_string(),
                severity: PublishSeverity::Warn,
                message: format!(
                    "maps never snapshotted — run travel snapshot-maps --plan-id {plan_id} --dest {destination}"
                ),
            });
        }
        Ok(crate::check_maps_fresh::Status::Stale { snapshotted_at }) => {
            issues.push(PublishIssue {
                category: "maps-fresh".to_string(),
                severity: PublishSeverity::Warn,
                message: format!(
                    "itinerary changed since maps snapshotted ({snapshotted_at}) — maps STALE, re-run travel snapshot-maps"
                ),
            });
        }
        Ok(crate::check_maps_fresh::Status::Fresh { .. }) => {}
        Err(_) => {}
    }

    // Maps completeness (WARN) — map_artifacts must hold both plan overviews
    // (plus one day-N.png per itinerary day). Reuses the doctor completeness
    // classifier: a key is OK only when status='uploaded' and byte_size > 64.
    match crate::check_maps_fresh::evaluate_completeness(&conn, plan_id).await {
        Ok(crate::check_maps_fresh::CompletenessVerdict::NoManifest) => {
            issues.push(PublishIssue {
                category: "maps-complete".to_string(),
                severity: PublishSeverity::Warn,
                message: format!(
                    "no map manifest — plan.png not uploaded; run travel snapshot-maps --plan-id {plan_id} --dest {destination}"
                ),
            });
        }
        Ok(crate::check_maps_fresh::CompletenessVerdict::Incomplete { line }) => {
            issues.push(PublishIssue {
                category: "maps-complete".to_string(),
                severity: PublishSeverity::Warn,
                message: line,
            });
        }
        Ok(crate::check_maps_fresh::CompletenessVerdict::Complete { .. }) => {}
        Err(_) => {}
    }

    // Weather (WARN within window; INFO when trip wholly beyond forecast window)
    if check_zh_weather {
        let missing_weather_days =
            missing_weather_in_window(&conn, plan_id, &destination, &today).await?;
        for day in &missing_weather_days {
            issues.push(PublishIssue {
                category: "weather".to_string(),
                severity: PublishSeverity::Warn,
                message: format!("day {day} missing weather within 16-day forecast window"),
            });
        }
        if missing_weather_days.is_empty() && day_count > 0 {
            if let Some(start_date) = trip_start_date(&conn, plan_id, &destination).await? {
                if start_beyond_forecast_window(&start_date, &today)? {
                    issues.push(PublishIssue {
                        category: "weather".to_string(),
                        severity: PublishSeverity::Info,
                        message: format!(
                            "weather pending - Open-Meteo forecast available within 16 days of {start_date}"
                        ),
                    });
                }
            }
        }
    }

    // AI-recommended provenance (INFO) — not gated by is_past
    let ai_activities = scalar_i64(
        &conn,
        "SELECT COUNT(*) FROM activities WHERE plan_id = ?1 AND destination = ?2 AND source = 'ai_recommended'",
        plan_id,
        &destination,
    )
    .await?;
    let ai_meals = scalar_i64(
        &conn,
        "SELECT COUNT(*) FROM session_meals WHERE plan_id = ?1 AND destination = ?2 AND source = 'ai_recommended'",
        plan_id,
        &destination,
    )
    .await?;
    let ai_routes = scalar_i64(
        &conn,
        "SELECT COUNT(*) FROM day_route_segments WHERE plan_id = ?1 AND destination = ?2 AND source = 'ai_recommended'",
        plan_id,
        &destination,
    )
    .await?;
    let ai_total = ai_activities + ai_meals + ai_routes;
    if ai_total > 0 {
        issues.push(PublishIssue {
            category: "ai-recommended".to_string(),
            severity: PublishSeverity::Info,
            message: format!(
                "{ai_total} AI-recommended item(s) awaiting confirmation ({ai_activities} activities, {ai_meals} meals, {ai_routes} routes)"
            ),
        });
    }

    if !is_past {
        for row in content_depth_rows(&conn, plan_id, &destination).await? {
            let missing_lunch = row.noon_meals == 0;
            let missing_dinner = row.evening_meals == 0;
            if missing_lunch || missing_dinner {
                let missing = match (missing_lunch, missing_dinner) {
                    (true, true) => "missing lunch meal (noon) and dinner meal (evening)",
                    (true, false) => "missing lunch meal (noon)",
                    (false, true) => "missing dinner meal (evening)",
                    (false, false) => unreachable!(),
                };
                let severity = if row.day_type == "arrival" || row.day_type == "departure" {
                    PublishSeverity::Info
                } else {
                    PublishSeverity::Warn
                };
                issues.push(PublishIssue {
                    category: "content-depth".to_string(),
                    severity,
                    message: format!("day {} may be thin: {missing}", row.day_number),
                });
            }
            if row.activities >= 2 && row.routes == 0 {
                issues.push(PublishIssue {
                    category: "content-depth".to_string(),
                    severity: PublishSeverity::Warn,
                    message: format!(
                        "day {} may be thin: {} activities but 0 route segments",
                        row.day_number, row.activities
                    ),
                });
            }
        }
    }

    Ok(issues)
}

async fn read_lifecycle(
    conn: &libsql::Connection,
    plan_id: &str,
    destination: &str,
) -> Result<Option<String>, String> {
    let mut rows = conn
        .query(
            "SELECT start_date, end_date FROM date_anchors WHERE plan_id = ?1 AND destination = ?2",
            params![plan_id.to_string(), destination.to_string()],
        )
        .await
        .map_err(|e| format!("date_anchors query failed: {e}"))?;
    if let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("date_anchors row read failed: {e}"))?
    {
        let start: String = row.get(0).unwrap_or_default();
        let end: String = row.get(1).unwrap_or_default();
        if !start.is_empty() && !end.is_empty() {
            let today = crate::plans::today_iso_pub();
            return Ok(Some(
                crate::plans::lifecycle(&start, &end, &today).to_string(),
            ));
        }
    }
    Ok(None)
}

/// Day-variant invariant violations (spec 2026-11-08-day-variants). `scope` = one plan+destination,
/// or `None` for every plan. Each message names the plan, destination and day, and contains
/// "variant". Invariants: `variant_of` points at an existing trip day of the same plan+destination
/// (no chain, no self-reference); a variant's date/day_type equal its main day's; a variant and a
/// main day that has variants carry non-empty labels (EN 1-24 chars of A-Za-z0-9 space . : - +, ZH 1-16 chars).
async fn variant_violations(
    conn: &libsql::Connection,
    scope: Option<(&str, &str)>,
) -> Result<Vec<String>, String> {
    let (plan, dest) = scope.unwrap_or(("", ""));
    let all = i64::from(scope.is_none());
    let mut out = Vec::new();

    let mut rows = conn
        .query(
            "SELECT v.plan_id, v.destination, v.day_number, v.variant_of, v.variant_label, \
                    v.variant_label_zh, v.date, v.day_type, \
                    m.day_number, m.variant_of, m.date, m.day_type \
             FROM days v LEFT JOIN days m ON m.plan_id = v.plan_id \
                  AND m.destination = v.destination AND m.day_number = v.variant_of \
             WHERE v.variant_of IS NOT NULL AND (?3 = 1 OR (v.plan_id = ?1 AND v.destination = ?2)) \
             ORDER BY v.plan_id, v.destination, v.day_number",
            params![plan.to_string(), dest.to_string(), all],
        )
        .await
        .map_err(|e| format!("variant invariant query failed: {e}"))?;
    while let Some(r) = rows
        .next()
        .await
        .map_err(|e| format!("variant invariant row read failed: {e}"))?
    {
        let g = |e: libsql::Error| format!("variant invariant decode failed: {e}");
        let (p, d): (String, String) = (r.get(0).map_err(g)?, r.get(1).map_err(g)?);
        let day: i64 = r.get(2).map_err(g)?;
        let parent: i64 = r.get(3).map_err(g)?;
        let label: Option<String> = r.get(4).map_err(g)?;
        let label_zh: Option<String> = r.get(5).map_err(g)?;
        let (date, day_type): (String, String) = (r.get(6).map_err(g)?, r.get(7).map_err(g)?);
        let main_day: Option<i64> = r.get(8).map_err(g)?;
        let main_of: Option<i64> = r.get(9).map_err(g)?;
        let main_date: Option<String> = r.get(10).map_err(g)?;
        let main_type: Option<String> = r.get(11).map_err(g)?;
        let at = format!("{p} ({d}) day {day}");
        if parent == day {
            out.push(format!("{at}: variant_of points at itself"));
        } else if main_day.is_none() {
            out.push(format!("{at}: variant_of {parent} does not exist"));
        } else if main_of.is_some() {
            out.push(format!("{at}: variant_of {parent} is itself a variant (chains are not allowed)"));
        } else {
            if main_date.as_deref() != Some(date.as_str()) {
                out.push(format!(
                    "{at}: variant date {date} differs from main day {parent} date {}",
                    main_date.unwrap_or_default()
                ));
            }
            if main_type.as_deref() != Some(day_type.as_str()) {
                out.push(format!(
                    "{at}: variant day_type {day_type} differs from main day {parent} day_type {}",
                    main_type.unwrap_or_default()
                ));
            }
        }
        out.extend(variant_label_problems(&at, "variant", label.as_deref(), label_zh.as_deref()));
    }

    let mut rows = conn
        .query(
            "SELECT DISTINCT m.plan_id, m.destination, m.day_number, m.variant_label, m.variant_label_zh \
             FROM days m JOIN days v ON v.plan_id = m.plan_id AND v.destination = m.destination \
                  AND v.variant_of = m.day_number \
             WHERE m.variant_of IS NULL AND (?3 = 1 OR (m.plan_id = ?1 AND m.destination = ?2)) \
             ORDER BY m.plan_id, m.destination, m.day_number",
            params![plan.to_string(), dest.to_string(), all],
        )
        .await
        .map_err(|e| format!("variant main-label query failed: {e}"))?;
    while let Some(r) = rows
        .next()
        .await
        .map_err(|e| format!("variant main-label row read failed: {e}"))?
    {
        let g = |e: libsql::Error| format!("variant main-label decode failed: {e}");
        let (p, d): (String, String) = (r.get(0).map_err(g)?, r.get(1).map_err(g)?);
        let day: i64 = r.get(2).map_err(g)?;
        let label: Option<String> = r.get(3).map_err(g)?;
        let label_zh: Option<String> = r.get(4).map_err(g)?;
        out.extend(variant_label_problems(
            &format!("{p} ({d}) day {day}"),
            "main day with variants",
            label.as_deref(),
            label_zh.as_deref(),
        ));
    }
    Ok(out)
}

fn variant_label_problems(at: &str, who: &str, en: Option<&str>, zh: Option<&str>) -> Vec<String> {
    let mut out = Vec::new();
    match en.map(str::trim).filter(|s| !s.is_empty()) {
        None => out.push(format!("{at}: {who} has no variant_label")),
        Some(_) => {
            if let Some(p) = crate::day_variant::en_label_problem(en.unwrap_or("")) {
                out.push(format!("{at}: {who} variant_label {p}"));
            }
        }
    }
    match zh.map(str::trim).filter(|s| !s.is_empty()) {
        None => out.push(format!("{at}: {who} has no variant_label_zh")),
        Some(z) => {
            if z.chars().count() > 16 {
                out.push(format!("{at}: {who} variant_label_zh must be 1-16 chars"));
            }
        }
    }
    out
}

/// One `plan_documents` (plan_id, doc_key, sha256) row for the M2 data rule.
#[derive(Debug, Clone)]
struct PlanDocSha {
    plan_id: String,
    doc_key: String,
    sha256: Option<String>,
}

/// The sha256 prefix length the voucher `?v=` link keys on: the worker's
/// offline/dashboard voucher links are built as `?v=<sha256 first 12>` —
/// `workers/trip-dashboard-rs/src/render/summary.rs` `document.sha256.get(..12)`
/// — so a shorter hash yields no version key at all.
const VOUCHER_SHA_KEY_LEN: usize = 12;

/// Pure rule (audit M2, data half): WARN when a document's sha256 cannot give
/// the voucher `?v=` link its 12-char key — the cached/local PDF then never
/// carries a stable version key. Mirrors the worker exactly
/// (`workers/trip-dashboard-rs/src/render/summary.rs`
/// `document.sha256.get(..12)`): the trimmed sha must slice at byte 12
/// (a non-ASCII char before that boundary makes the worker's `get` return
/// None — no `?v=` at all) and the first 12 chars must be hex (only then can
/// the value come from a real sha256). NULL/empty are warned too. One line per
/// offending document.
fn short_sha256_lines(docs: &[PlanDocSha]) -> Vec<String> {
    docs.iter()
        .filter_map(|d| {
            // The worker slices the RAW value (`sha256.get(..12)`, no trim), so
            // padding is judged as stored; only an all-blank value counts as empty.
            let trimmed = d.sha256.as_deref().filter(|s| !s.trim().is_empty());
            let problem = match trimmed {
                Some(sha) if sha.len() < VOUCHER_SHA_KEY_LEN => {
                    format!("'{sha}' ({} chars)", sha.len())
                }
                Some(sha) => match sha.get(..VOUCHER_SHA_KEY_LEN) {
                    // Same byte-slice as the worker's `?v=` key build.
                    None => format!(
                        "'{sha}' — its first {VOUCHER_SHA_KEY_LEN} bytes do not end on a \
                         character boundary (non-ASCII inside)"
                    ),
                    Some(key) if !key.bytes().all(|b| b.is_ascii_hexdigit()) => {
                        format!("'{sha}' — its first {VOUCHER_SHA_KEY_LEN} chars are not hex")
                    }
                    Some(_) => return None,
                },
                None if d.sha256.is_none() => "NULL".to_string(),
                None => "empty".to_string(),
            };
            Some(format!(
                "{} document {}: sha256 is {problem} — the voucher ?v= link keys on the \
                 first {VOUCHER_SHA_KEY_LEN} hex chars, so this document's cached PDF never gets \
                 a stable version; re-upload the document",
                d.plan_id, d.doc_key
            ))
        })
        .collect()
}

async fn validate_plan_documents(issues: &mut Vec<Issue>) {
    let conn = match db::connect_read().await {
        Ok(c) => c,
        Err(e) => {
            issues.push(Issue {
                category: "plan-documents".to_string(),
                severity: Severity::Error,
                message: format!("cannot check plan documents: {e}"),
                file: Some("turso:plan_documents".to_string()),
                line: None,
            });
            return;
        }
    };
    let result = async {
        let mut rows = conn.query(
            "SELECT pd.plan_id, pd.doc_key, pd.day_number, p.plan_id, \
                    EXISTS(SELECT 1 FROM days d WHERE d.plan_id = pd.plan_id AND d.day_number = pd.day_number), \
                    EXISTS(SELECT 1 FROM days d WHERE d.plan_id = pd.plan_id AND d.day_number = pd.day_number AND d.variant_of IS NULL) \
             FROM plan_document_days pd \
             LEFT JOIN plan_documents p ON p.plan_id = pd.plan_id AND p.doc_key = pd.doc_key \
             ORDER BY pd.plan_id, pd.doc_key, pd.day_number",
            (),
        ).await.map_err(|e| format!("plan document day query failed: {e}"))?;
        while let Some(row) = rows.next().await.map_err(|e| format!("plan document day row failed: {e}"))? {
            let plan: String = row.get(0).map_err(|e| e.to_string())?;
            let key: String = row.get(1).map_err(|e| e.to_string())?;
            let day: i64 = row.get(2).map_err(|e| e.to_string())?;
            let parent: Option<String> = row.get(3).map_err(|e| e.to_string())?;
            let day_exists: i64 = row.get(4).map_err(|e| e.to_string())?;
            let trip_day_exists: i64 = row.get(5).map_err(|e| e.to_string())?;
            let at = format!("{plan} document {key} day {day}");
            if parent.is_none() {
                issues.push(Issue {
                    category: "plan-documents".to_string(),
                    severity: Severity::Error,
                    message: format!("{at}: day row has no parent plan_documents row"),
                    file: Some("turso:plan_document_days".to_string()),
                    line: None,
                });
            }
            if day_exists == 0 || trip_day_exists == 0 {
                issues.push(Issue {
                    category: "plan-documents".to_string(),
                    severity: Severity::Error,
                    message: if day_exists == 0 {
                        format!("{at}: referenced day does not exist")
                    } else {
                        format!("{at}: referenced day is a variant, not a trip day")
                    },
                    file: Some("turso:plan_document_days".to_string()),
                    line: None,
                });
            }
        }
        // r2_key shape rule + audit M2 sha256 rule share ONE targeted SELECT
        // through the travel-db repo reader (DAL boundary) — rules in Rust.
        let docs = travel_db::repo::plan_documents::list_key_rows(&conn)
            .await
            .map_err(|e| format!("plan document key rows failed: {e}"))?;
        for d in &docs {
            let expected = format!("{}/doc-{}.pdf", d.plan_id, d.doc_key);
            if d.r2_key != expected {
                issues.push(Issue {
                    category: "plan-documents".to_string(),
                    severity: Severity::Error,
                    message: format!(
                        "{} document {}: r2_key {:?} must be {:?}",
                        d.plan_id, d.doc_key, d.r2_key, expected
                    ),
                    file: Some("turso:plan_documents".to_string()),
                    line: None,
                });
            }
        }
        let sha_docs: Vec<PlanDocSha> = docs
            .iter()
            .map(|d| PlanDocSha {
                plan_id: d.plan_id.clone(),
                doc_key: d.doc_key.clone(),
                sha256: d.sha256.clone(),
            })
            .collect();
        for message in short_sha256_lines(&sha_docs) {
            issues.push(Issue {
                category: "plan-documents".to_string(),
                severity: Severity::Warning,
                message,
                file: Some("turso:plan_documents".to_string()),
                line: None,
            });
        }
        Ok::<(), String>(())
    }.await;
    if let Err(message) = result {
        issues.push(Issue {
            category: "plan-documents".to_string(),
            severity: Severity::Error,
            message,
            file: Some("turso:plan_documents".to_string()),
            line: None,
        });
    }
}

/// `validate data`: the dashboard FIT section's booked framing must match the
/// data (Warning). A plan booked through ingest-known (set-flight / set-hotel)
/// has no plan_offer_selection, so unless one listed FIT carries the booked
/// marker the page frames a booked trip as 優先評估，非已訂. Conversely a booked
/// marker while P4 is not booked claims a booking that does not exist.
/// A check that cannot run is an ERROR — a silent skip hides the drift.
async fn validate_fit_booking(issues: &mut Vec<Issue>) {
    let push = |issues: &mut Vec<Issue>, severity: Severity, message: String| {
        issues.push(Issue {
            category: "fit-offers".to_string(),
            severity,
            message,
            file: Some("turso:plan_fit_offers".to_string()),
            line: None,
        })
    };
    let conn = match db::connect_read().await {
        Ok(c) => c,
        Err(e) => {
            push(issues, Severity::Error, format!("cannot check FIT booked markers: {e}"));
            return;
        }
    };
    match travel_db::repo::plan_fit_offers::booking_states(&conn).await {
        Ok(states) => {
            for state in &states {
                for message in fit_booking_warnings(state) {
                    push(issues, Severity::Warning, message);
                }
            }
        }
        Err(e) if e.contains("no such column") => push(
            issues,
            Severity::Error,
            format!(
                "cannot check FIT booked markers: plan_fit_offers.booked is missing — \
                 run ./bin/travel db migrate ({e})"
            ),
        ),
        Err(e) => push(issues, Severity::Error, format!("cannot check FIT booked markers: {e}")),
    }
}

/// `validate data`: every process row of a live plan must hold a status on its
/// own ladder (cascade::status_machine — Anchor P1/P2, Booking P3_4/P3/P4,
/// Itinerary P5). Off-ladder statuses and unknown process ids are Errors, so
/// drift fails loud. A check that cannot run is an Error too.
async fn validate_process_status_ladders(issues: &mut Vec<Issue>) {
    let push = |issues: &mut Vec<Issue>, message: String| {
        issues.push(Issue {
            category: "process-status".to_string(),
            severity: Severity::Error,
            message,
            file: Some("turso:process_statuses".to_string()),
            line: None,
        })
    };
    let conn = match db::connect_read().await {
        Ok(c) => c,
        Err(e) => {
            push(issues, format!("cannot check process status ladders: {e}"));
            return;
        }
    };
    match travel_db::repo::process_statuses::list_live(&conn).await {
        Ok(rows) => {
            for row in &rows {
                if let Some(message) = off_ladder_message(row) {
                    push(issues, message);
                }
            }
        }
        Err(e) => push(issues, format!("cannot check process status ladders: {e}")),
    }
}

/// Pure predicate behind `validate_process_status_ladders`.
fn off_ladder_message(row: &travel_db::repo::process_statuses::ProcessStatusRow) -> Option<String> {
    use crate::cascade::status_machine::{ladder, on_ladder};
    let at = format!("{} / {} {}", row.plan_id, row.destination, row.process_id);
    let l = match ladder(&row.process_id) {
        Ok(l) => l,
        Err(e) => return Some(format!("{at}: {e}")),
    };
    if on_ladder(l, &row.status) {
        return None;
    }
    let shown = if row.status.is_empty() { "<empty>" } else { row.status.as_str() };
    Some(format!(
        "{at}: status {shown} is not on the {} ladder — move it with set-process-status",
        l.name()
    ))
}

/// Pure predicate behind `validate_fit_booking`: the warning lines for one
/// plan+destination's FIT-list state.
fn fit_booking_warnings(state: &travel_db::repo::plan_fit_offers::FitBookingState) -> Vec<String> {
    let at = format!("{} / {}", state.plan_id, state.destination);
    // `confirmed` is past `booked` on the ladder (the worker treats both as booked).
    let p4_booked = matches!(state.p4_status.as_str(), "booked" | "confirmed");
    let mut out = Vec::new();
    if p4_booked && state.listed > 0 && state.booked == 0 && !state.selection_matches {
        out.push(format!(
            "{at}: FIT list present but no FIT is marked booked; the dashboard shows it as \
             優先評估，非已訂 — run set-fit-offer <offer_id> --booked --plan-id {} --dest {}, \
             or remove the stale FIT list",
            state.plan_id, state.destination
        ));
    }
    if state.booked > 0 && !p4_booked {
        let p4 = if state.p4_status.is_empty() { "unset" } else { state.p4_status.as_str() };
        out.push(format!(
            "{at}: a FIT is marked booked but P4 (process_4_accommodation) is {p4} — \
             run set-fit-offer <offer_id> --not-booked --plan-id {} --dest {}, or book P4",
            state.plan_id, state.destination
        ));
    }
    out
}

/// `validate data`: day-variant invariants across every plan (Error).
async fn validate_day_variants(issues: &mut Vec<Issue>) {
    let conn = match db::connect_read().await {
        Ok(c) => c,
        Err(e) => {
            issues.push(Issue {
                category: "day-variants".to_string(),
                severity: Severity::Error,
                message: format!("cannot check day variants: {e}"),
                file: Some("turso:days".to_string()),
                line: None,
            });
            return;
        }
    };
    match variant_violations(&conn, None).await {
        Ok(msgs) => {
            for message in msgs {
                issues.push(Issue {
                    category: "day-variants".to_string(),
                    severity: Severity::Error,
                    message,
                    file: Some("turso:days".to_string()),
                    line: None,
                });
            }
        }
        Err(message) => issues.push(Issue {
            category: "day-variants".to_string(),
            severity: Severity::Error,
            message,
            file: Some("turso:days".to_string()),
            line: None,
        }),
    }
}

async fn count_days(
    conn: &libsql::Connection,
    plan_id: &str,
    destination: &str,
) -> Result<i64, String> {
    let mut rows = conn
        .query(
            "SELECT COUNT(*) FROM days WHERE plan_id = ?1 AND destination = ?2 AND variant_of IS NULL",
            params![plan_id.to_string(), destination.to_string()],
        )
        .await
        .map_err(|e| format!("days count query failed: {e}"))?;
    if let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("days count row read failed: {e}"))?
    {
        return Ok(row.get(0).unwrap_or(0));
    }
    Ok(0)
}

async fn missing_day_zh(
    conn: &libsql::Connection,
    plan_id: &str,
    destination: &str,
) -> Result<Vec<i64>, String> {
    let sql = "SELECT d.day_number \
        FROM days d \
        WHERE d.plan_id = ?1 AND d.destination = ?2 \
          AND ( \
            EXISTS ( \
              SELECT 1 FROM activities a \
              WHERE a.plan_id = d.plan_id AND a.destination = d.destination \
                AND a.day_number = d.day_number \
            ) \
            OR EXISTS ( \
              SELECT 1 FROM session_meals m \
              WHERE m.plan_id = d.plan_id AND m.destination = d.destination \
                AND m.day_number = d.day_number \
            ) \
            OR EXISTS ( \
              SELECT 1 FROM day_route_segments r \
              WHERE r.plan_id = d.plan_id AND r.destination = d.destination \
                AND r.day_number = d.day_number \
            ) \
          ) \
          AND NULLIF(TRIM(COALESCE(d.theme_zh, '')), '') IS NULL";
    query_day_numbers(conn, sql, plan_id, destination).await
}

async fn missing_session_zh(
    conn: &libsql::Connection,
    plan_id: &str,
    destination: &str,
) -> Result<Vec<(i64, String)>, String> {
    let sql = "SELECT t.day_number, t.session_type \
        FROM timesofday t \
        WHERE t.plan_id = ?1 AND t.destination = ?2 \
          AND ( \
            EXISTS ( \
              SELECT 1 FROM activities a \
              WHERE a.plan_id = t.plan_id AND a.destination = t.destination \
                AND a.day_number = t.day_number AND a.session_type = t.session_type \
            ) \
            OR EXISTS ( \
              SELECT 1 FROM session_meals m \
              WHERE m.plan_id = t.plan_id AND m.destination = t.destination \
                AND m.day_number = t.day_number AND m.session_type = t.session_type \
            ) \
            OR NULLIF(TRIM(COALESCE(t.transit_notes, '')), '') IS NOT NULL \
            OR NULLIF(TRIM(COALESCE(t.transit_notes_zh, '')), '') IS NOT NULL \
          ) \
          AND NULLIF(TRIM(COALESCE(t.focus_zh, '')), '') IS NULL \
          AND NULLIF(TRIM(COALESCE(t.transit_notes_zh, '')), '') IS NULL";
    let mut rows = conn
        .query(
            sql,
            params![plan_id.to_string(), destination.to_string()],
        )
        .await
        .map_err(|e| format!("missing session zh query failed: {e}"))?;
    let mut out = Vec::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("missing session zh row read failed: {e}"))?
    {
        let day: i64 = row.get(0).unwrap_or(0);
        let session: String = row.get(1).unwrap_or_default();
        out.push((day, session));
    }
    Ok(out)
}

async fn map_coverage_gaps(
    conn: &libsql::Connection,
    plan_id: &str,
    destination: &str,
) -> Result<Vec<i64>, String> {
    let sql = "WITH activity_days AS ( \
          SELECT day_number, COUNT(*) AS activity_count \
          FROM activities \
          WHERE plan_id = ?1 AND destination = ?2 \
          GROUP BY day_number \
        ), \
        mappable_days AS ( \
          SELECT a.day_number, COUNT(*) AS mappable_count \
          FROM activities a \
          JOIN destination_pois p \
            ON p.slug = a.destination AND p.poi_id = a.poi_id \
          WHERE a.plan_id = ?1 AND a.destination = ?2 \
            AND p.lat IS NOT NULL AND p.lon IS NOT NULL \
            AND TRIM(CAST(p.lat AS TEXT)) <> '' \
            AND TRIM(CAST(p.lon AS TEXT)) <> '' \
          GROUP BY a.day_number \
        ), \
        route_days AS ( \
          SELECT day_number, COUNT(*) AS route_count \
          FROM day_route_segments \
          WHERE plan_id = ?1 AND destination = ?2 \
          GROUP BY day_number \
        ) \
        SELECT ad.day_number \
        FROM activity_days ad \
        LEFT JOIN mappable_days md USING (day_number) \
        LEFT JOIN route_days rd USING (day_number) \
        WHERE COALESCE(md.mappable_count, 0) = 0 \
          AND COALESCE(rd.route_count, 0) = 0 \
        ORDER BY ad.day_number";
    query_day_numbers(conn, sql, plan_id, destination).await
}

async fn has_map_path(
    conn: &libsql::Connection,
    plan_id: &str,
    destination: &str,
) -> Result<bool, String> {
    let poi_sql = "SELECT COUNT(*) \
        FROM activities a \
        JOIN destination_pois p ON p.slug = a.destination AND p.poi_id = a.poi_id \
        WHERE a.plan_id = ?1 AND a.destination = ?2 \
          AND p.lat IS NOT NULL AND p.lon IS NOT NULL \
          AND TRIM(CAST(p.lat AS TEXT)) <> '' \
          AND TRIM(CAST(p.lon AS TEXT)) <> ''";
    let route_sql =
        "SELECT COUNT(*) FROM day_route_segments WHERE plan_id = ?1 AND destination = ?2";
    let poi_count = scalar_i64(conn, poi_sql, plan_id, destination).await?;
    if poi_count > 0 {
        return Ok(true);
    }
    let route_count = scalar_i64(conn, route_sql, plan_id, destination).await?;
    Ok(route_count > 0)
}

async fn missing_weather_in_window(
    conn: &libsql::Connection,
    plan_id: &str,
    destination: &str,
    today: &str,
) -> Result<Vec<i64>, String> {
    let sql = "SELECT day_number \
        FROM days \
        WHERE plan_id = ?1 AND destination = ?2 AND variant_of IS NULL \
          AND date >= ?3 AND date <= date(?3, '+16 days') \
          AND (weather_label IS NULL OR weather_source_id IS NULL)";
    let mut rows = conn
        .query(
            sql,
            params![
                plan_id.to_string(),
                destination.to_string(),
                today.to_string()
            ],
        )
        .await
        .map_err(|e| format!("weather window query failed: {e}"))?;
    let mut out = Vec::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("weather window row read failed: {e}"))?
    {
        out.push(row.get(0).unwrap_or(0));
    }
    Ok(out)
}

async fn trip_start_date(
    conn: &libsql::Connection,
    plan_id: &str,
    destination: &str,
) -> Result<Option<String>, String> {
    let mut rows = conn
        .query(
            "SELECT start_date FROM date_anchors WHERE plan_id = ?1 AND destination = ?2",
            params![plan_id.to_string(), destination.to_string()],
        )
        .await
        .map_err(|e| format!("date_anchors start_date query failed: {e}"))?;
    if let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("date_anchors start_date row read failed: {e}"))?
    {
        let start: String = row.get(0).unwrap_or_default();
        return Ok((!start.is_empty()).then_some(start));
    }
    Ok(None)
}

fn start_beyond_forecast_window(start_date: &str, today: &str) -> Result<bool, String> {
    let start = chrono::NaiveDate::parse_from_str(start_date, "%Y-%m-%d")
        .map_err(|e| format!("invalid trip start_date {start_date}: {e}"))?;
    let today = chrono::NaiveDate::parse_from_str(today, "%Y-%m-%d")
        .map_err(|e| format!("invalid today date {today}: {e}"))?;
    Ok((start - today).num_days() > 16)
}

async fn query_day_numbers(
    conn: &libsql::Connection,
    sql: &str,
    plan_id: &str,
    destination: &str,
) -> Result<Vec<i64>, String> {
    let mut rows = conn
        .query(
            sql,
            params![plan_id.to_string(), destination.to_string()],
        )
        .await
        .map_err(|e| format!("day zh query failed: {e}"))?;
    let mut out = Vec::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("day zh row read failed: {e}"))?
    {
        out.push(row.get(0).unwrap_or(0));
    }
    Ok(out)
}

struct ContentDepthRow {
    day_number: i64,
    day_type: String,
    activities: i64,
    noon_meals: i64,
    evening_meals: i64,
    routes: i64,
}

async fn content_depth_rows(
    conn: &libsql::Connection,
    plan_id: &str,
    destination: &str,
) -> Result<Vec<ContentDepthRow>, String> {
    let sql = "WITH day_rows AS (SELECT day_number, day_type FROM days WHERE plan_id=?1 AND destination=?2),
      activity_counts AS (SELECT day_number, COUNT(*) AS activities FROM activities WHERE plan_id=?1 AND destination=?2 GROUP BY day_number),
      meal_counts AS (SELECT day_number, SUM(CASE WHEN session_type='noon' THEN 1 ELSE 0 END) AS noon_meals, SUM(CASE WHEN session_type='evening' THEN 1 ELSE 0 END) AS evening_meals FROM session_meals WHERE plan_id=?1 AND destination=?2 GROUP BY day_number),
      route_counts AS (SELECT day_number, COUNT(*) AS routes FROM day_route_segments WHERE plan_id=?1 AND destination=?2 GROUP BY day_number)
      SELECT d.day_number, d.day_type, COALESCE(a.activities,0), COALESCE(m.noon_meals,0), COALESCE(m.evening_meals,0), COALESCE(r.routes,0)
      FROM day_rows d
      LEFT JOIN activity_counts a ON a.day_number=d.day_number
      LEFT JOIN meal_counts m ON m.day_number=d.day_number
      LEFT JOIN route_counts r ON r.day_number=d.day_number
      ORDER BY d.day_number";
    let mut rows = conn
        .query(
            sql,
            params![plan_id.to_string(), destination.to_string()],
        )
        .await
        .map_err(|e| format!("content_depth query failed: {e}"))?;
    let mut out = Vec::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| e.to_string())?
    {
        out.push(ContentDepthRow {
            day_number: row.get(0).map_err(|e| e.to_string())?,
            day_type: row.get(1).map_err(|e| e.to_string())?,
            activities: row.get(2).unwrap_or(0),
            noon_meals: row.get(3).unwrap_or(0),
            evening_meals: row.get(4).unwrap_or(0),
            routes: row.get(5).unwrap_or(0),
        });
    }
    Ok(out)
}

async fn scalar_i64(
    conn: &libsql::Connection,
    sql: &str,
    plan_id: &str,
    destination: &str,
) -> Result<i64, String> {
    let mut rows = conn
        .query(
            sql,
            params![plan_id.to_string(), destination.to_string()],
        )
        .await
        .map_err(|e| format!("scalar query failed: {e}"))?;
    if let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("scalar row read failed: {e}"))?
    {
        return Ok(row.get(0).unwrap_or(0));
    }
    Ok(0)
}

fn emit_publish_report(issues: &[PublishIssue]) {
    let blockers: Vec<&PublishIssue> = issues
        .iter()
        .filter(|i| i.severity == PublishSeverity::Block)
        .collect();
    let warnings: Vec<&PublishIssue> = issues
        .iter()
        .filter(|i| i.severity == PublishSeverity::Warn)
        .collect();
    let infos: Vec<&PublishIssue> = issues
        .iter()
        .filter(|i| i.severity == PublishSeverity::Info)
        .collect();

    if !blockers.is_empty() {
        println!("## Blockers\n");
        for r in &blockers {
            println!("  ❌ [{}] {}", r.category, r.message);
        }
        println!();
    }
    if !warnings.is_empty() {
        println!("## Warnings\n");
        for r in &warnings {
            println!("  ⚠️  [{}] {}", r.category, r.message);
        }
        println!();
    }
    if !infos.is_empty() {
        println!("## Info\n");
        for r in &infos {
            println!("  ℹ️  [{}] {}", r.category, r.message);
        }
        println!();
    }

    println!("## Summary\n");
    println!("  Blockers:   {}", blockers.len());
    println!("  Warnings:   {}", warnings.len());
    println!("  Info:       {}", infos.len());
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
mod record_columns_tests {
    use super::*;
    use travel_db::repo::records::RecordLintRow;

    fn rrow(
        table: &str,
        plan: &str,
        dest: &str,
        day: i64,
        session: Option<&str>,
        text: &str,
        status: Option<&str>,
        date: Option<&str>,
    ) -> RecordLintRow {
        RecordLintRow {
            table: table.to_string(),
            plan_id: plan.to_string(),
            destination: dest.to_string(),
            day,
            session: session.map(str::to_string),
            row_text: text.to_string(),
            status: status.map(str::to_string),
            // a status None in fixtures means a summary row (timesofday)
            is_summary: status.is_none(),
            date: date.map(str::to_string),
            variant_of: None,
        }
    }

    #[test]
    fn off_ladder_record_status_values_error_with_full_row_identity() {
        let rows = vec![
            rrow("activities", "kyoto-nov-2026", "kyoto_nov_2026", 4, Some("morning"), "東福寺", Some("cancelled"), Some("2026-11-19")),
            rrow("session_meals", "kyoto-nov-2026", "kyoto_nov_2026", 5, Some("noon"), "京極スタンド", Some("done but drifted"), None),
            rrow("day_route_segments", "kyoto-nov-2026", "kyoto_nov_2026", 5, None, "東寺 → 京極スタンド", Some("DONE"), Some("2026-11-20")),
        ];
        let lines = invalid_record_status_lines(&rows);
        assert_eq!(lines.len(), 3, "{lines:?}");
        // every line names plan / day / session / row
        assert!(lines[0].contains("kyoto-nov-2026") && lines[0].contains("day 4") && lines[0].contains("morning"));
        assert!(lines[0].contains("activities") && lines[0].contains("東福寺") && lines[0].contains("'cancelled'"));
        assert!(lines[1].contains("session_meals") && lines[1].contains("京極スタンド") && lines[1].contains("'done but drifted'"));
        assert!(lines[2].contains("day_route_segments") && lines[2].contains("東寺 → 京極スタンド") && lines[2].contains("'DONE'"));
    }

    #[test]
    fn done_and_skipped_are_the_only_legal_non_null_statuses() {
        let rows = vec![
            rrow("activities", "p", "d", 1, Some("morning"), "東寺", Some("done"), Some("2026-11-01")),
            rrow("session_meals", "p", "d", 1, Some("noon"), "蕎麥", Some("skipped"), None),
            rrow("day_route_segments", "p", "d", 2, None, "A → B", Some("done"), Some("2026-11-02")),
        ];
        assert!(invalid_record_status_lines(&rows).is_empty());
    }

    #[test]
    fn future_recorded_days_warn_but_today_and_past_do_not() {
        let rows = vec![
            rrow("activities", "kyoto-nov-2026", "kyoto_nov_2026", 4, Some("morning"), "東福寺", Some("done"), Some("2026-11-19")),
            rrow("day_route_segments", "kyoto-nov-2026", "kyoto_nov_2026", 5, None, "A → B", Some("skipped"), Some("2026-11-18")),
            rrow("session_meals", "kyoto-nov-2026", "kyoto_nov_2026", 3, Some("noon"), "丼", Some("done"), Some("2026-11-17")),
        ];
        let lines = future_recorded_lines(&rows, "kyoto_nov_2026", "2026-11-18");
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].contains("day 4") && lines[0].contains("morning") && lines[0].contains("2026-11-19"));
        assert!(lines[0].contains("after the destination-local today 2026-11-18"));
    }

    #[test]
    fn future_rule_is_scoped_to_the_named_destination() {
        let rows = vec![
            rrow("activities", "p", "kyoto_nov_2026", 4, Some("morning"), "東福寺", Some("done"), Some("2026-11-19")),
            rrow("activities", "p", "taipei_2026", 1, Some("morning"), "故宮", Some("done"), Some("2026-11-19")),
        ];
        let lines = future_recorded_lines(&rows, "taipei_2026", "2026-11-18");
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].contains("taipei_2026") && lines[0].contains("故宮"));
    }

    #[test]
    fn recorded_row_without_a_day_date_warns_it_cannot_be_date_checked() {
        let rows = vec![
            rrow("activities", "p", "d", 9, Some("evening"), "夜訪", Some("done"), None),
            rrow("activities", "p", "d", 10, Some("morning"), "早市", Some("skipped"), Some("")),
        ];
        let lines = future_recorded_lines(&rows, "d", "2026-11-18");
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines[0].contains("day 9") && lines[0].contains("no date"));
        assert!(lines[1].contains("day 10") && lines[1].contains("no date"));
    }

    // Round 2 (K3.1 review #1): a session summary (timesofday.record_summary_zh,
    // a complete record on its own — spec §5 record-session) on a future day is
    // future content marked as actual and must WARN like any recorded row.
    #[test]
    fn session_summary_on_a_future_day_warns() {
        let rows = vec![
            rrow("timesofday", "kyoto-nov-2026", "kyoto_nov_2026", 6, Some("morning"),
                 "人多到走不動，改成先去南禪寺", None, Some("2026-11-20")),
            rrow("timesofday", "kyoto-nov-2026", "kyoto_nov_2026", 5, Some("evening"),
                 "夜訪當天結束得早", None, Some("2026-11-19")),
        ];
        let lines = future_recorded_lines(&rows, "kyoto_nov_2026", "2026-11-19");
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].contains("day 6") && lines[0].contains("morning"));
        assert!(lines[0].contains("session summary") && lines[0].contains("人多到走不動"));
        assert!(lines[0].contains("2026-11-20") && lines[0].contains("2026-11-19"));
    }

    // Round 2 (K3.1 review #3): spec §5 — alternatives (variant days) are not
    // recorded; a done/skipped row OR a summary on a variant day is a WARN.
    #[test]
    fn recorded_row_or_summary_on_a_variant_day_warns() {
        let mut status_row = rrow(
            "activities", "kyoto-nov-2026", "kyoto_nov_2026", 5, Some("morning"),
            "雨備上常寂光寺", Some("done"), Some("2026-11-16"),
        );
        status_row.variant_of = Some(3);
        let mut summary_row = rrow(
            "timesofday", "kyoto-nov-2026", "kyoto_nov_2026", 5, Some("noon"),
            "當天中午下雨，改成博物館", None, Some("2026-11-16"),
        );
        summary_row.variant_of = Some(3);
        let main_day_row = rrow(
            "activities", "kyoto-nov-2026", "kyoto_nov_2026", 3, Some("morning"),
            "東寺", Some("done"), Some("2026-11-14"),
        ); // variant_of stays None — a real trip day, no warning
        let lines = variant_recorded_lines(&[status_row, summary_row, main_day_row]);
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines[0].contains("day 5") && lines[0].contains("morning"));
        assert!(lines[0].contains("雨備上常寂光寺") && lines[0].contains("variant_of 3"));
        assert!(lines[1].contains("day 5") && lines[1].contains("noon"));
        assert!(lines[1].contains("session summary") && lines[1].contains("當天中午下雨"));
    }
}

#[cfg(test)]
mod plan_document_sha_tests {
    use super::*;

    fn doc(plan: &str, key: &str, sha: Option<&str>) -> PlanDocSha {
        PlanDocSha {
            plan_id: plan.to_string(),
            doc_key: key.to_string(),
            sha256: sha.map(str::to_string),
        }
    }

    #[test]
    fn null_empty_and_short_sha256_warn() {
        let docs = vec![
            doc("kyoto-nov-2026", "doc-flight", None),
            doc("kyoto-nov-2026", "doc-hotel", Some("")),
            doc("okinawa-2026", "doc-tour", Some("abc12345678")),
        ];
        let lines = short_sha256_lines(&docs);
        assert_eq!(lines.len(), 3, "{lines:?}");
        assert!(lines[0].contains("kyoto-nov-2026") && lines[0].contains("doc-flight") && lines[0].contains("NULL"));
        assert!(lines[1].contains("doc-hotel") && lines[1].contains("empty"));
        assert!(lines[2].contains("okinawa-2026") && lines[2].contains("doc-tour") && lines[2].contains("abc12345678"));
        assert!(lines.iter().all(|l| l.contains("12")));
    }

    #[test]
    fn twelve_char_sha_is_the_minimum_key() {
        let docs = vec![
            doc("p", "d1", Some("0123456789ab")),
            doc("p", "d2", Some("0123456789abcdef0123456789abcdef")),
        ];
        assert!(short_sha256_lines(&docs).is_empty());
    }

    #[test]
    fn padded_sha_warns_because_the_worker_slices_the_raw_value() {
        // " 0123456789ab " → the worker's get(..12) is " 0123456789a" (not hex).
        let docs = vec![doc("p", "d3", Some(" 0123456789ab "))];
        assert_eq!(short_sha256_lines(&docs).len(), 1);
    }

    // Round 2 (K3.1 review #2): match the worker's `sha256.get(..12)` exactly.
    // "01234567890é" is 12 chars / 13 bytes — byte 12 is inside the é, so the
    // worker's get(..12) returns None and the voucher link gets NO ?v= key; the
    // old `len() < 12` check stayed silent about it.
    #[test]
    fn non_ascii_sha_13_bytes_long_warns_though_len_is_13() {
        let docs = vec![doc("kyoto-nov-2026", "doc-flight", Some("01234567890é"))];
        let lines = short_sha256_lines(&docs);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].contains("kyoto-nov-2026") && lines[0].contains("doc-flight"));
        assert!(lines[0].contains("01234567890é"));
    }

    // Round 2 (K3.1 review #2): the ?v= key must be hex — a non-hex 12+ char
    // string slices fine but can never come from a real sha256.
    #[test]
    fn non_hex_sha_warns_even_when_12_chars_are_sliceable() {
        let docs = vec![
            doc("p", "d1", Some("zzzzzzzzzzzzzzzz")), // sliceable, not hex
            doc("p", "d2", Some("ABCDEF0123456789")), // uppercase hex is still hex
            doc("p", "d3", Some("01234567890z")), // 12 chars, 12 bytes, one non-hex char
        ];
        let lines = short_sha256_lines(&docs);
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines[0].contains("d1") && lines[0].contains("not hex"));
        assert!(lines[1].contains("d3") && lines[1].contains("not hex"));
    }
}

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
    fn complete_docs_pass() {
        let good = "DEFAULT: known-flights fast-path (create plan → set-flight/set-hotel → itinerary).\n\
            Stage 2 modes: shop | ingest-known | defer. transport/accommodation VALIDATION is mandatory in every mode.";
        assert!(planning_flow_doc_violations(good).is_empty());
    }

    #[test]
    fn missing_fast_path_flagged() {
        let bad = "Stage 2 modes: shop | ingest-known | defer. VALIDATION is mandatory in every mode.";
        let v = planning_flow_doc_violations(bad);
        assert!(v.iter().any(|m| m.contains("known-flights fast-path")), "{v:?}");
    }

    #[test]
    fn missing_a_mode_flagged() {
        let bad = "known-flights fast-path is the default.\n\
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

#[cfg(test)]
mod fit_booking_tests {
    use super::*;
    use travel_db::repo::plan_fit_offers::FitBookingState;

    fn state(p4: &str, listed: i64, booked: i64, selection_matches: bool) -> FitBookingState {
        FitBookingState {
            plan_id: "kyoto-nov-2026".into(),
            destination: "kyoto_nov_2026".into(),
            p4_status: p4.into(),
            listed,
            booked,
            selection_matches,
        }
    }

    fn ps_row(process_id: &str, status: &str) -> travel_db::repo::process_statuses::ProcessStatusRow {
        travel_db::repo::process_statuses::ProcessStatusRow {
            plan_id: "p".to_string(),
            destination: "d".to_string(),
            process_id: process_id.to_string(),
            status: status.to_string(),
        }
    }

    #[test]
    fn off_ladder_statuses_are_flagged() {
        assert!(off_ladder_message(&ps_row("process_5_daily_itinerary", "researched")).is_none());
        assert!(off_ladder_message(&ps_row("process_4_accommodation", "cancelled")).is_none());
        assert!(off_ladder_message(&ps_row("process_1_date_anchor", "confirmed")).is_none());
        let m = off_ladder_message(&ps_row("process_5_daily_itinerary", "booked")).unwrap();
        assert!(m.contains("not on the itinerary ladder"), "{m}");
        let m = off_ladder_message(&ps_row("process_2_destination", "selecting")).unwrap();
        assert!(m.contains("not on the anchor ladder"), "{m}");
        assert!(off_ladder_message(&ps_row("process_3_transportation", "archived")).is_some());
        assert!(off_ladder_message(&ps_row("process_3_transportation", "")).is_some());
        let m = off_ladder_message(&ps_row("process_9_bogus", "pending")).unwrap();
        assert!(m.contains("unknown process id"), "{m}");
    }

    #[test]
    fn booked_p4_with_unmarked_fit_list_warns() {
        let w = fit_booking_warnings(&state("booked", 1, 0, false));
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("no FIT is marked booked"), "{w:?}");
        assert!(w[0].contains("優先評估，非已訂"), "{w:?}");
        assert!(w[0].contains("--booked --plan-id kyoto-nov-2026 --dest kyoto_nov_2026"), "{w:?}");
    }

    #[test]
    fn marker_or_matching_selection_satisfies_booked_p4() {
        assert!(fit_booking_warnings(&state("booked", 2, 1, false)).is_empty());
        assert!(fit_booking_warnings(&state("booked", 2, 0, true)).is_empty());
        // A confirmed P4 is still booked: the marker is valid, its absence still warns.
        assert!(fit_booking_warnings(&state("confirmed", 1, 1, false)).is_empty());
        assert_eq!(fit_booking_warnings(&state("confirmed", 1, 0, false)).len(), 1);
    }

    #[test]
    fn unbooked_p4_without_marker_is_fine() {
        assert!(fit_booking_warnings(&state("selecting", 2, 0, false)).is_empty());
        assert!(fit_booking_warnings(&state("", 2, 0, false)).is_empty());
        // No listed offer the page would show → no misleading heading to warn on.
        assert!(fit_booking_warnings(&state("booked", 0, 0, false)).is_empty());
    }

    #[test]
    fn marker_without_booked_p4_warns() {
        let w = fit_booking_warnings(&state("selecting", 1, 1, false));
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("marked booked but P4"), "{w:?}");
        assert!(w[0].contains("is selecting"), "{w:?}");
        let w = fit_booking_warnings(&state("", 1, 1, false));
        assert!(w[0].contains("is unset"), "{w:?}");
    }
}
