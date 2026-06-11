//! validate-itinerary — port of src/cli/commands/validate.ts + the
//! ItineraryValidator (src/validation/itinerary-validator.ts). READ-ONLY.
//!
//! Loads days + activities from the normalized tables for one destination,
//! builds per-day summaries (the TS buildDaySummaries), runs every validator
//! check, then prints the report. Exit code 1 when errors remain after the
//! --severity filter, else 0. No DB writes.
//!
//! Validator checks ported 1:1:
//!   - time conflicts (overlapping start/end)        → error
//!   - day packing (>12h, >3/session, empty non-arr) → warning/info
//!   - booking deadlines (passed / approaching / no-deadline) → error/warn/info
//!   - business hours (from "Hours: HH:MM-HH:MM" in notes) → warning
//!   - area efficiency (A→B→A, >4 unique areas)      → info
//!   - logical order (later time in earlier session) → warning

use crate::db;
use libsql::{params, Connection};

const BOOKING_WARNING_DAYS: i64 = 7;
const MAX_ACTIVITIES_PER_SESSION: usize = 3;
const MAX_HOURS_PER_DAY: i64 = 12;

const SESSIONS: &[&str] = &["morning", "noon", "afternoon", "evening"];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Severity {
    Error,
    Warning,
    Info,
}

impl Severity {
    fn rank(self) -> u8 {
        match self {
            Severity::Error => 3,
            Severity::Warning => 2,
            Severity::Info => 1,
        }
    }
    fn prefix(self) -> &'static str {
        match self {
            Severity::Error => "X",
            Severity::Warning => "!",
            Severity::Info => "i",
        }
    }
    fn label(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Info => "info",
        }
    }
}

struct Issue {
    severity: Severity,
    day: Option<i64>,
    session: Option<String>,
    message: String,
    suggestion: Option<String>,
}

struct Activity {
    title: String,
    session: String,
    start_time: Option<String>,
    end_time: Option<String>,
    duration_min: i64,
    area: Option<String>,
    booking_required: bool,
    booking_status: Option<String>,
    book_by: Option<String>,
    operating_hours: Option<String>,
}

struct DaySummary {
    day_number: i64,
    date: String,
    theme: String,
    activities: Vec<Activity>,
    total_duration_min: i64,
}

pub async fn run(args: &[String], plan_id: String) -> Result<(), String> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!(
            "Usage:\n  travel validate-itinerary [--dest <slug>] [--severity error|warning|info]"
        );
        return Ok(());
    }
    let dest_opt = option_value(args, "--dest");
    let severity_opt = option_value(args, "--severity");

    let threshold = parse_severity(severity_opt.as_deref())?;

    let conn = db::connect_read().await?;
    let destination = read_destination(&conn, &plan_id, dest_opt.as_deref()).await?;

    let days = load_day_summaries(&conn, &plan_id, &destination).await?;
    if days.is_empty() {
        eprintln!("Error: No itinerary days found. Run scaffold-itinerary first.");
        std::process::exit(1);
    }

    let issues = validate(&days);

    let (errors, warnings, info) = count(&issues);
    let valid = errors == 0;

    let filtered: Vec<&Issue> = issues
        .iter()
        .filter(|i| i.severity.rank() >= threshold.rank())
        .collect();

    print_result(&destination, valid, errors, warnings, info, threshold, &filtered);

    if valid {
        Ok(())
    } else {
        std::process::exit(1);
    }
}

fn count(issues: &[Issue]) -> (usize, usize, usize) {
    let mut e = 0;
    let mut w = 0;
    let mut i = 0;
    for x in issues {
        match x.severity {
            Severity::Error => e += 1,
            Severity::Warning => w += 1,
            Severity::Info => i += 1,
        }
    }
    (e, w, i)
}

// ── validator ────────────────────────────────────────────────────────

fn validate(days: &[DaySummary]) -> Vec<Issue> {
    let mut issues = Vec::new();
    for day in days {
        validate_day_conflicts(day, &mut issues);
        validate_day_packing(day, &mut issues);
        validate_booking_deadlines(day, &mut issues);
        validate_business_hours(day, &mut issues);
        validate_area_efficiency(day, &mut issues);
        check_map_links(day, &mut issues);
    }
    validate_logical_order(days, &mut issues);
    issues
}

fn validate_day_conflicts(day: &DaySummary, out: &mut Vec<Issue>) {
    let timed: Vec<&Activity> = day
        .activities
        .iter()
        .filter(|a| a.start_time.is_some() && a.end_time.is_some())
        .collect();
    for i in 0..timed.len() {
        let a1 = timed[i];
        let s1 = parse_time(a1.start_time.as_ref().unwrap());
        let e1 = parse_time(a1.end_time.as_ref().unwrap());
        for a2 in timed.iter().skip(i + 1) {
            let s2 = parse_time(a2.start_time.as_ref().unwrap());
            let e2 = parse_time(a2.end_time.as_ref().unwrap());
            if let (Some(s1), Some(e1), Some(s2), Some(e2)) = (s1, e1, s2, e2) {
                if s1 < e2 && s2 < e1 {
                    out.push(Issue {
                        severity: Severity::Error,
                        day: Some(day.day_number),
                        session: None,
                        message: format!(
                            "Time conflict: \"{}\" ({}-{}) overlaps with \"{}\" ({}-{})",
                            a1.title,
                            a1.start_time.as_ref().unwrap(),
                            a1.end_time.as_ref().unwrap(),
                            a2.title,
                            a2.start_time.as_ref().unwrap(),
                            a2.end_time.as_ref().unwrap()
                        ),
                        suggestion: Some("Adjust timing for one of these activities".to_string()),
                    });
                }
            }
        }
    }
}

fn validate_day_packing(day: &DaySummary, out: &mut Vec<Issue>) {
    let max_minutes = MAX_HOURS_PER_DAY * 60;
    if day.total_duration_min > max_minutes {
        out.push(Issue {
            severity: Severity::Warning,
            day: Some(day.day_number),
            session: None,
            message: format!(
                "Day {} has {} hours of activities (max: {}h)",
                day.day_number,
                ((day.total_duration_min as f64) / 60.0).round() as i64,
                MAX_HOURS_PER_DAY
            ),
            suggestion: Some("Consider moving some activities to another day".to_string()),
        });
    }

    // Per-session counts, in first-seen order.
    let mut order: Vec<String> = Vec::new();
    let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for a in &day.activities {
        let c = counts.entry(a.session.clone()).or_insert(0);
        if *c == 0 {
            order.push(a.session.clone());
        }
        *c += 1;
    }
    for session in &order {
        let count = counts[session];
        if count > MAX_ACTIVITIES_PER_SESSION {
            out.push(Issue {
                severity: Severity::Warning,
                day: Some(day.day_number),
                session: Some(session.clone()),
                message: format!(
                    "{session} has {count} activities (max: {MAX_ACTIVITIES_PER_SESSION})"
                ),
                suggestion: Some("Spread activities across sessions".to_string()),
            });
        }
    }

    let theme_lc = day.theme.to_lowercase();
    let is_arr_dep = theme_lc.contains("arrival") || theme_lc.contains("departure");
    if !is_arr_dep && day.activities.is_empty() {
        out.push(Issue {
            severity: Severity::Info,
            day: Some(day.day_number),
            session: None,
            message: format!("Day {} has no activities planned", day.day_number),
            suggestion: Some("Add activities or mark as rest day".to_string()),
        });
    }
}

fn validate_booking_deadlines(day: &DaySummary, out: &mut Vec<Issue>) {
    let today = today_civil_date();
    let today_str = format!("{:04}-{:02}-{:02}", today.0, today.1, today.2);
    // Historical days are not actionable.
    if is_iso_date(&day.date) && day.date < today_str {
        return;
    }
    let today_days = days_from_civil(today.0, today.1, today.2);

    for a in &day.activities {
        if !a.booking_required {
            continue;
        }
        let status = a.booking_status.clone().unwrap_or_else(|| "pending".to_string());
        if status == "booked" || status == "not_required" {
            continue;
        }
        if let Some(book_by) = a.book_by.as_ref().filter(|s| !s.is_empty()) {
            if let Some((y, m, d)) = parse_iso_date(book_by) {
                let deadline_days = days_from_civil(y, m, d);
                let days_until = deadline_days - today_days;
                if days_until < 0 {
                    out.push(Issue {
                        severity: Severity::Error,
                        day: Some(day.day_number),
                        session: None,
                        message: format!(
                            "Booking deadline PASSED for \"{}\" (was {})",
                            a.title, book_by
                        ),
                        suggestion: Some("Book immediately or remove activity".to_string()),
                    });
                } else if days_until <= BOOKING_WARNING_DAYS {
                    out.push(Issue {
                        severity: Severity::Warning,
                        day: Some(day.day_number),
                        session: None,
                        message: format!(
                            "Booking deadline in {} day(s) for \"{}\" ({})",
                            days_until, a.title, book_by
                        ),
                        suggestion: Some("Complete booking soon".to_string()),
                    });
                }
            }
        } else if status == "pending" {
            out.push(Issue {
                severity: Severity::Info,
                day: Some(day.day_number),
                session: None,
                message: format!(
                    "\"{}\" requires booking but has no deadline set",
                    a.title
                ),
                suggestion: Some("Set a book_by date to track deadline".to_string()),
            });
        }
    }
}

fn validate_business_hours(day: &DaySummary, out: &mut Vec<Issue>) {
    for a in &day.activities {
        let (Some(start), Some(hours)) = (a.start_time.as_ref(), a.operating_hours.as_ref()) else {
            continue;
        };
        let Some((open, close)) = parse_operating_hours(hours) else {
            continue;
        };
        let Some(activity_start) = parse_time(start) else {
            continue;
        };
        let activity_end = match a.end_time.as_ref().and_then(|t| parse_time(t)) {
            Some(e) => e,
            None => activity_start + a.duration_min,
        };
        if activity_start < open {
            out.push(Issue {
                severity: Severity::Warning,
                day: Some(day.day_number),
                session: None,
                message: format!(
                    "\"{}\" starts at {} but opens at {}",
                    a.title, start, format_time(open)
                ),
                suggestion: Some(format!("Start at or after {}", format_time(open))),
            });
        }
        if activity_end > close {
            out.push(Issue {
                severity: Severity::Warning,
                day: Some(day.day_number),
                session: None,
                message: format!(
                    "\"{}\" ends at {} but closes at {}",
                    a.title,
                    format_time(activity_end),
                    format_time(close)
                ),
                suggestion: Some(format!("Plan to finish by {}", format_time(close))),
            });
        }
    }
}

fn validate_area_efficiency(day: &DaySummary, out: &mut Vec<Issue>) {
    if day.activities.len() < 3 {
        return;
    }
    let area_seq: Vec<String> = day
        .activities
        .iter()
        .filter_map(|a| a.area.clone())
        .collect();
    if area_seq.len() < 3 {
        return;
    }
    let mut i = 0;
    while i + 2 < area_seq.len() {
        if area_seq[i] == area_seq[i + 2] && area_seq[i] != area_seq[i + 1] {
            out.push(Issue {
                severity: Severity::Info,
                day: Some(day.day_number),
                session: None,
                message: format!(
                    "Day {} has back-and-forth travel: {} → {} → {}",
                    day.day_number,
                    area_seq[i],
                    area_seq[i + 1],
                    area_seq[i + 2]
                ),
                suggestion: Some("Reorder activities to minimize transit".to_string()),
            });
            break;
        }
        i += 1;
    }
    let unique: std::collections::HashSet<&String> = area_seq.iter().collect();
    if unique.len() > 4 {
        out.push(Issue {
            severity: Severity::Info,
            day: Some(day.day_number),
            session: None,
            message: format!(
                "Day {} covers {} different areas",
                day.day_number,
                unique.len()
            ),
            suggestion: Some("Consider clustering activities by area".to_string()),
        });
    }
}

fn validate_logical_order(days: &[DaySummary], out: &mut Vec<Issue>) {
    for day in days {
        let mut sorted: Vec<&Activity> = day
            .activities
            .iter()
            .filter(|a| a.start_time.is_some())
            .collect();
        sorted.sort_by_key(|a| parse_time(a.start_time.as_ref().unwrap()).unwrap_or(0));
        for w in sorted.windows(2) {
            let curr = w[0];
            let next = w[1];
            let cso = session_order(&curr.session);
            let nso = session_order(&next.session);
            if cso > nso {
                out.push(Issue {
                    severity: Severity::Warning,
                    day: Some(day.day_number),
                    session: None,
                    message: format!(
                        "\"{}\" ({}) is scheduled before \"{}\" ({}) but has later time",
                        curr.title, curr.session, next.title, next.session
                    ),
                    suggestion: Some(
                        "Verify session assignments match actual times".to_string(),
                    ),
                });
            }
        }
    }
}

fn session_order(session: &str) -> usize {
    SESSIONS.iter().position(|s| *s == session).unwrap_or(0)
}

// ── map-link lint ─────────────────────────────────────────────────────
//
// Catches the activity-text shape that breaks the dashboard's per-stop Google
// Maps "search" links: a MULTI-LINE blob that ALSO embeds an http(s) URL. The
// worker's maps/search/<query> fallback URL-encodes the whole title, so a
// newline becomes %0A and the embedded URL becomes a nested https%3A — garbage.
// The fix is render_activity_text in the worker (renders the embedded URL as a
// clean inline labeled link); this lint flags the stored data so the bug is
// caught at the data/validate layer too, not just visually. Advisory (warning),
// never an error → it must not fail `validate-itinerary` or `doctor`.
fn check_map_links(day: &DaySummary, out: &mut Vec<Issue>) {
    for a in &day.activities {
        if is_malformed_map_text(&a.title) {
            out.push(Issue {
                severity: Severity::Warning,
                day: Some(day.day_number),
                session: Some(a.session.clone()),
                message: format!(
                    "activity text has embedded URL + newlines — will produce a malformed map link; ensure the worker uses render_activity_text: \"{}\"",
                    truncate(&a.title, 50)
                ),
                suggestion: Some(
                    "The worker's render_activity_text turns the embedded \"Google Maps：<url>\" tail into a clean labeled link; verify the dashboard is deployed with it.".to_string(),
                ),
            });
        }
    }
}

/// The malformed-map-link predicate: text that will become a Google-Maps
/// `search` link is malformed when it contains BOTH a newline AND an embedded
/// http(s) URL (the multi-line-blob-with-nested-URL pattern). Pure + testable.
fn is_malformed_map_text(text: &str) -> bool {
    let has_newline = text.contains('\n');
    let has_url = text.contains("http://") || text.contains("https://");
    has_newline && has_url
}

/// Truncate a string to at most `max` chars, appending an ellipsis when cut.
/// Newlines are shown as the literal "\n" so the one-line message stays readable.
fn truncate(s: &str, max: usize) -> String {
    let flat = s.replace('\n', "\\n");
    let chars: Vec<char> = flat.chars().collect();
    if chars.len() <= max {
        flat
    } else {
        let head: String = chars[..max].iter().collect();
        format!("{head}…")
    }
}

/// Doctor hook: run the map-link lint for ONE plan's active destination and
/// return (day_number, message) for each malformed-map-link warning. Read-only,
/// advisory — `doctor` renders these as warnings (exit 0). Returns an empty vec
/// on any DB/load error (the lint never blocks doctor).
pub async fn map_link_errors(plan_id: &str) -> Vec<(i64, String)> {
    let Ok(conn) = db::connect_read().await else { return Vec::new() };
    let Ok(dest) = read_destination(&conn, plan_id, None).await else { return Vec::new() };
    let Ok(days) = load_day_summaries(&conn, plan_id, &dest).await else { return Vec::new() };
    let mut issues: Vec<Issue> = Vec::new();
    for day in &days {
        check_map_links(day, &mut issues);
    }
    issues
        .into_iter()
        .map(|i| (i.day.unwrap_or(0), i.message))
        .collect()
}

// ── data loading ─────────────────────────────────────────────────────

async fn load_day_summaries(
    conn: &Connection,
    plan_id: &str,
    dest: &str,
) -> Result<Vec<DaySummary>, String> {
    // Days for the destination.
    let mut day_rows = conn
        .query(
            "SELECT day_number, date, theme, day_type FROM days \
             WHERE plan_id = ?1 AND destination = ?2 ORDER BY day_number",
            params![plan_id.to_string(), dest.to_string()],
        )
        .await
        .map_err(|e| format!("days query failed: {e}"))?;

    let mut summaries: Vec<DaySummary> = Vec::new();
    while let Some(r) = day_rows
        .next()
        .await
        .map_err(|e| format!("days row read failed: {e}"))?
    {
        let day_number: i64 = r.get(0).unwrap_or(0);
        let date: String = r.get(1).unwrap_or_default();
        let theme_opt: Option<String> = r.get(2).ok().flatten();
        let day_type: String = r.get(3).unwrap_or_default();
        let theme = theme_opt.filter(|t| !t.is_empty()).unwrap_or(day_type);
        summaries.push(DaySummary {
            day_number,
            date,
            theme,
            activities: Vec::new(),
            total_duration_min: 0,
        });
    }

    // Activities for the destination, grouped by day/session.
    let mut act_rows = conn
        .query(
            "SELECT day_number, session_type, sort_order, title, area, duration_min, \
                    booking_required, booking_status, book_by, start_time, end_time, notes \
             FROM activities WHERE plan_id = ?1 AND destination = ?2 \
             ORDER BY day_number, \
               CASE session_type WHEN 'morning' THEN 0 WHEN 'noon' THEN 1 \
                 WHEN 'afternoon' THEN 2 ELSE 3 END, sort_order",
            params![plan_id.to_string(), dest.to_string()],
        )
        .await
        .map_err(|e| format!("activities query failed: {e}"))?;

    while let Some(r) = act_rows
        .next()
        .await
        .map_err(|e| format!("activities row read failed: {e}"))?
    {
        let day_number: i64 = r.get(0).unwrap_or(0);
        let session: String = r.get(1).unwrap_or_default();
        let title: String = r.get(3).unwrap_or_default();
        let area: Option<String> = r.get(4).ok().flatten();
        let duration_min_raw: Option<i64> = r.get(5).ok().flatten();
        let booking_required: i64 = r.get(6).unwrap_or(0);
        let booking_status: Option<String> = r.get(7).ok().flatten();
        let book_by: Option<String> = r.get(8).ok().flatten();
        let start_time: Option<String> =
            r.get(9).ok().flatten().filter(|s: &String| !s.is_empty());
        let end_time: Option<String> =
            r.get(10).ok().flatten().filter(|s: &String| !s.is_empty());
        let notes: Option<String> = r.get(11).ok().flatten();

        let duration_min = infer_duration(duration_min_raw, &start_time, &end_time);

        let Some(summary) = summaries.iter_mut().find(|d| d.day_number == day_number) else {
            // Activity for a day with no day row — skip (TS only iterates days).
            continue;
        };
        summary.total_duration_min += duration_min;
        summary.activities.push(Activity {
            title,
            session,
            start_time,
            end_time,
            duration_min,
            area: area.filter(|s| !s.is_empty()),
            booking_required: booking_required != 0,
            booking_status,
            book_by,
            operating_hours: parse_hours_from_notes(notes.as_deref()),
        });
    }

    Ok(summaries)
}

fn infer_duration(
    duration_min: Option<i64>,
    start: &Option<String>,
    end: &Option<String>,
) -> i64 {
    if let Some(d) = duration_min {
        if d > 0 {
            return d;
        }
    }
    if let (Some(s), Some(e)) = (start.as_ref(), end.as_ref()) {
        if let (Some(sm), Some(em)) = (parse_time(s), parse_time(e)) {
            if em > sm {
                return em - sm;
            }
        }
    }
    60
}

fn parse_hours_from_notes(notes: Option<&str>) -> Option<String> {
    let notes = notes?;
    // /(?:^|\s)Hours:\s*([^|\n]+)/i
    let lower = notes.to_lowercase();
    let idx = lower.find("hours:")?;
    // Ensure it's at start or preceded by whitespace.
    if idx != 0 {
        let prev = notes[..idx].chars().last();
        if !matches!(prev, Some(c) if c.is_whitespace()) {
            return None;
        }
    }
    let after = &notes[idx + "hours:".len()..];
    let value: String = after
        .chars()
        .take_while(|&c| c != '|' && c != '\n')
        .collect();
    let v = value.trim();
    if v.is_empty() {
        None
    } else {
        Some(v.to_string())
    }
}

// ── time helpers ─────────────────────────────────────────────────────

fn parse_time(t: &str) -> Option<i64> {
    let (h, m) = t.split_once(':')?;
    let hh: i64 = h.trim().parse().ok()?;
    let mm: i64 = m.trim().parse().ok()?;
    Some(hh * 60 + mm)
}

fn format_time(minutes: i64) -> String {
    let h = minutes / 60;
    let m = minutes % 60;
    format!("{h:02}:{m:02}")
}

fn parse_operating_hours(hours: &str) -> Option<(i64, i64)> {
    let lower = hours.to_lowercase();
    if hours == "24h" || lower == "always" {
        return None;
    }
    // Match (\d{1,2}:\d{2})\s*[-–]\s*(\d{1,2}:\d{2})
    let chars: Vec<char> = hours.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if let Some((open, ni)) = scan_hhmm(&chars, i) {
            let mut j = ni;
            while j < chars.len() && chars[j].is_whitespace() {
                j += 1;
            }
            if j < chars.len() && (chars[j] == '-' || chars[j] == '–') {
                j += 1;
                while j < chars.len() && chars[j].is_whitespace() {
                    j += 1;
                }
                if let Some((close, _)) = scan_hhmm(&chars, j) {
                    return Some((open, close));
                }
            }
        }
        i += 1;
    }
    None
}

/// Scan `H:MM` or `HH:MM` at `start`; return (minutes, end_index).
fn scan_hhmm(chars: &[char], start: usize) -> Option<(i64, usize)> {
    let mut i = start;
    let h0 = i;
    while i < chars.len() && chars[i].is_ascii_digit() && i - h0 < 2 {
        i += 1;
    }
    if i == h0 || i >= chars.len() || chars[i] != ':' {
        return None;
    }
    let hh: i64 = chars[h0..i].iter().collect::<String>().parse().ok()?;
    i += 1; // skip ':'
    let m0 = i;
    while i < chars.len() && chars[i].is_ascii_digit() && i - m0 < 2 {
        i += 1;
    }
    if i - m0 != 2 {
        return None;
    }
    let mm: i64 = chars[m0..i].iter().collect::<String>().parse().ok()?;
    Some((hh * 60 + mm, i))
}

fn is_iso_date(value: &str) -> bool {
    let b = value.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b[..4].iter().all(|c| c.is_ascii_digit())
        && b[5..7].iter().all(|c| c.is_ascii_digit())
        && b[8..10].iter().all(|c| c.is_ascii_digit())
}

fn parse_iso_date(value: &str) -> Option<(i32, u32, u32)> {
    if !is_iso_date(value) {
        return None;
    }
    let y: i32 = value[0..4].parse().ok()?;
    let m: u32 = value[5..7].parse().ok()?;
    let d: u32 = value[8..10].parse().ok()?;
    Some((y, m, d))
}

/// Days since the Unix epoch (proleptic Gregorian) — used for day-delta math.
fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y } as i64;
    let m = m as i64;
    let d = d as i64;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn today_civil_date() -> (i32, u32, u32) {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let days = secs.div_euclid(86_400);
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
    (y as i32, m as u32, d as u32)
}

// ── report ───────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn print_result(
    destination: &str,
    valid: bool,
    errors: usize,
    warnings: usize,
    info: usize,
    threshold: Severity,
    filtered: &[&Issue],
) {
    let status = if valid { "VALID" } else { "ISSUES FOUND" };
    println!("\nvalidate-itinerary ({destination})");
    println!("   Result: {status}");
    println!("   Showing: {}+", threshold.label());
    println!("   Summary: {errors} error(s), {warnings} warning(s), {info} info");

    if filtered.is_empty() {
        println!("\n(no issues to show)\n");
        return;
    }

    println!("\nIssues:");
    for i in filtered {
        let where_parts: Vec<String> = [i.day.map(|d| format!("Day {d}")), i.session.clone()]
            .into_iter()
            .flatten()
            .collect();
        let where_str = where_parts.join(" ");
        let prefix = i.severity.prefix();
        if where_str.is_empty() {
            println!("  {prefix} {}", i.message);
        } else {
            println!("  {prefix} {where_str}: {}", i.message);
        }
        if let Some(s) = &i.suggestion {
            println!("     -> {s}");
        }
    }
    println!();
}

// ── arg helpers ──────────────────────────────────────────────────────

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

fn parse_severity(value: Option<&str>) -> Result<Severity, String> {
    match value {
        None => Ok(Severity::Info),
        Some(v) => match v.to_lowercase().as_str() {
            "error" => Ok(Severity::Error),
            "warning" => Ok(Severity::Warning),
            "info" => Ok(Severity::Info),
            _ => Err("Error: --severity must be one of: error | warning | info".to_string()),
        },
    }
}

async fn read_destination(
    conn: &Connection,
    plan_id: &str,
    dest_opt: Option<&str>,
) -> Result<String, String> {
    if let Some(d) = dest_opt {
        return Ok(d.to_string());
    }
    let mut rows = conn
        .query(
            "SELECT active_destination FROM plan_metadata WHERE plan_id = ?1",
            params![plan_id.to_string()],
        )
        .await
        .map_err(|e| format!("plan_metadata query failed: {e}"))?;
    if let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("plan_metadata row read failed: {e}"))?
    {
        let dest: String = row.get(0).unwrap_or_default();
        if dest.is_empty() {
            return Err("plan_metadata.active_destination is empty".to_string());
        }
        return Ok(dest);
    }
    Err(format!("plan_metadata row missing for plan_id={plan_id}"))
}

#[cfg(test)]
mod map_link_tests {
    use super::*;

    fn act(session: &str, title: &str) -> Activity {
        Activity {
            title: title.to_string(),
            session: session.to_string(),
            start_time: None,
            end_time: None,
            duration_min: 60,
            area: None,
            booking_required: false,
            booking_status: None,
            book_by: None,
            operating_hours: None,
        }
    }

    // The exact okinawa bug shape: a multi-line title with an embedded maps URL.
    #[test]
    fn malformed_when_newline_and_embedded_url() {
        let blob = "晚餐：ステーキ88 — 牧志駅步行5分\nGoogle Maps：https://www.google.com/maps/search/abc";
        assert!(is_malformed_map_text(blob));
    }

    #[test]
    fn malformed_driving_leg_with_nav_url() {
        let blob = "04:00 自家出發開車：紅樹林 → 大園\n地址：桃園市\nGoogle Maps 導航：https://www.google.com/maps/dir/A/B";
        assert!(is_malformed_map_text(blob));
    }

    // Clean single-line venue name — NOT malformed.
    #[test]
    fn clean_single_line_is_not_malformed() {
        assert!(!is_malformed_map_text("Naminoue Shrine"));
        assert!(!is_malformed_map_text("首里城公園"));
    }

    // Multi-line but no embedded URL — NOT malformed (newline alone is fine).
    #[test]
    fn multiline_without_url_is_not_malformed() {
        assert!(!is_malformed_map_text("晚餐：安里家\n營業：週五 17:00–23:00"));
    }

    // Single-line WITH a URL — NOT malformed (no newline → search query is clean;
    // and render_activity_text handles the inline link regardless).
    #[test]
    fn single_line_with_url_is_not_malformed() {
        assert!(!is_malformed_map_text("see https://example.com/x"));
    }

    // The lint emits a WARNING (advisory), with day + session, for a bad activity.
    #[test]
    fn lint_warns_on_malformed_activity() {
        let day = DaySummary {
            day_number: 2,
            date: "2026-06-13".into(),
            theme: "test".into(),
            total_duration_min: 60,
            activities: vec![act(
                "evening",
                "晚餐：安里家 — 飯店步行5分\nGoogle Maps：https://www.google.com/maps/search/x",
            )],
        };
        let mut out = Vec::new();
        check_map_links(&day, &mut out);
        assert_eq!(out.len(), 1, "expected exactly one warning");
        assert!(matches!(out[0].severity, Severity::Warning));
        assert_eq!(out[0].day, Some(2));
        assert_eq!(out[0].session.as_deref(), Some("evening"));
        assert!(out[0].message.contains("embedded URL + newlines"), "got: {}", out[0].message);
        assert!(out[0].message.contains("render_activity_text"), "got: {}", out[0].message);
        // Truncation flattens the newline to a literal \n (one-line message).
        assert!(!out[0].message.contains('\n'), "message must be single-line, got: {}", out[0].message);
    }

    // A clean day produces NO warnings.
    #[test]
    fn lint_silent_on_clean_activities() {
        let day = DaySummary {
            day_number: 1,
            date: "2026-06-12".into(),
            theme: "arrival".into(),
            total_duration_min: 60,
            activities: vec![
                act("morning", "Naminoue Shrine"),
                act("noon", "Makishi Market"),
            ],
        };
        let mut out = Vec::new();
        check_map_links(&day, &mut out);
        assert!(out.is_empty(), "clean activities must not warn, got {} issues", out.len());
    }

    #[test]
    fn truncate_flattens_newlines_and_caps_length() {
        assert_eq!(truncate("a\nb", 50), "a\\nb");
        let long = "x".repeat(100);
        let t = truncate(&long, 10);
        assert!(t.chars().count() <= 11, "10 chars + ellipsis, got {}", t.chars().count());
        assert!(t.ends_with('…'));
    }
}
