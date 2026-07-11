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
    // Map-link lint inputs (see validate_map_links).
    transit_chains: Vec<TransitChain>, // from timesofday.transit_notes(_zh), split on →
    route_legs: Vec<RouteLeg>,         // from day_route_segments
    activity_map_urls: Vec<String>,    // https://…maps… URLs embedded in activity titles
    meals: Vec<MealEntry>,             // session_meals rows (for pin-coverage check)
}

// One session_meals row (session + text) for the meal-pin coverage check.
struct MealEntry {
    session: String,
    text: String,
}

// One transit-pill string (e.g. "A→B→C") tied to its session, for link linting.
struct TransitChain {
    session: String,
    text: String,
}

// One day_route_segments row.
struct RouteLeg {
    from: String,
    to: String,
    mode: String,
}

/// Return itinerary validator ERROR-level messages for publish-readiness reuse.
/// READ-ONLY — no DB writes.
pub async fn error_messages(plan_id: &str, dest_opt: Option<&str>) -> Result<Vec<String>, String> {
    let conn = db::connect_read().await?;
    let destination = read_destination(&conn, plan_id, dest_opt).await?;
    let days = load_day_summaries(&conn, plan_id, &destination).await?;
    if days.is_empty() {
        return Ok(Vec::new());
    }
    let issues = validate(&days);
    Ok(issues
        .iter()
        .filter(|i| i.severity == Severity::Error)
        .map(|i| i.message.clone())
        .collect())
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

/// Run the map-link/meal checks for one plan's active destination and return ALL
/// issues. Shared loader for the doctor hooks below. Best-effort: empty on any
/// DB/resolution failure so doctor never hard-fails here.
async fn collect_map_link_issues(plan_id: &str) -> Vec<Issue> {
    let Ok(conn) = db::connect_read().await else {
        return Vec::new();
    };
    let Ok(destination) = read_destination(&conn, plan_id, None).await else {
        return Vec::new();
    };
    let Ok(days) = load_day_summaries(&conn, plan_id, &destination).await else {
        return Vec::new();
    };
    let mut issues = Vec::new();
    for day in &days {
        validate_map_links(day, &mut issues);
    }
    issues
}

/// Message prefix the reservation check emits — shared so the doctor hook can
/// pick those findings back out of the issue list without re-deriving them.
const RESERVATION_MSG_PREFIX: &str = "Restaurant may need a reservation";

/// Agent-first hook for `doctor`: run only the map-link checks for one plan's
/// active destination and return the ERROR-level findings (cross-country legs —
/// the ocean-spanning Maps routes). Returns (day, message) pairs.
pub async fn map_link_errors(plan_id: &str) -> Vec<(i64, String)> {
    collect_map_link_issues(plan_id)
        .await
        .into_iter()
        .filter(|i| matches!(i.severity, Severity::Error))
        .map(|i| (i.day.unwrap_or(0), i.message))
        .collect()
}

/// Agent-first hook for `doctor`: dashboard activity-title map-link warnings
/// from the dashboard branch. Kept narrow so generic advisory map-link warnings
/// still stay in `validate-itinerary`, while this renderer-dependent shape is
/// surfaced by doctor as before.
pub async fn malformed_map_link_warnings(plan_id: &str) -> Vec<(i64, String)> {
    collect_map_link_issues(plan_id)
        .await
        .into_iter()
        .filter(|i| {
            matches!(i.severity, Severity::Warning)
                && i.message
                    .starts_with("activity text has embedded URL + newlines")
        })
        .map(|i| (i.day.unwrap_or(0), i.message))
        .collect()
}

/// Agent-first hook for `doctor`: sit-down restaurants that may need a
/// reservation and are NOT yet enrolled in the booking ledger (no booked/pending
/// activity in their session). Self-clearing — once you `set-activity-booking …
/// pending|booked`, the finding disappears. Returns (day, restaurant-label).
pub async fn unbooked_reservations(plan_id: &str) -> Vec<(i64, String)> {
    collect_map_link_issues(plan_id)
        .await
        .into_iter()
        .filter(|i| i.message.starts_with(RESERVATION_MSG_PREFIX))
        .map(|i| (i.day.unwrap_or(0), i.message))
        .collect()
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
        validate_map_links(day, &mut issues);
    }
    validate_logical_order(days, &mut issues);
    issues
}

// ── map-link lint ─────────────────────────────────────────────────────
//
// Catches the classes of broken Google-Maps links we kept finding by hand:
//   1. activity-text map URL containing '&'  → the dashboard linkifier truncates
//      at the first '&', so query-param URLs break. Use the path form. (warning)
//   2. transit/route leg crossing countries (a Taiwan place + a Japan place in one
//      leg) → Maps draws an ocean-spanning route. (error)
//   3. a rail/bus leg (單軌/巴士/線/JR/電車…) that would be mode-detected as walking
//      because of a trailing 步行 note. (warning)
//   4. an ambiguous bare-name stop (e.g. 安里 with no 駅/站/Station and no spelled-out
//      city) that Maps can't geolocate reliably. (info)
fn validate_map_links(day: &DaySummary, out: &mut Vec<Issue>) {
    // Dashboard branch guard: multi-line activity text with an embedded URL
    // depends on the rich renderer to avoid raw/expanded Maps URLs.
    for a in &day.activities {
        if is_malformed_map_text(&a.title) {
            out.push(Issue {
                severity: Severity::Warning,
                day: Some(day.day_number),
                session: Some(a.session.clone()),
                message: format!(
                    "activity text has embedded URL + newlines — requires deployed render_activity_text to avoid an expanded/raw map URL: \"{}\"",
                    truncate(&a.title, 50)
                ),
                suggestion: Some(
                    "The Rust worker's render_activity_text turns the embedded \"Google Maps：<url>\" tail into a clean labeled link; verify deployed pages use that renderer.".to_string(),
                ),
            });
        }
    }

    // (1) activity-text URLs with '&'
    for url in &day.activity_map_urls {
        if url.contains('&') {
            out.push(Issue {
                severity: Severity::Warning,
                day: Some(day.day_number),
                session: None,
                message: format!(
                    "Map URL contains '&' and will be truncated by the dashboard linkifier: {}",
                    truncate(url, 60)
                ),
                suggestion: Some(
                    "Use the path form https://www.google.com/maps/dir/<from>/<to> (no & query params)".to_string(),
                ),
            });
        }
    }

    // Build the leg list from both transit chains and route segments.
    // Each leg = (from_text, to_text, full_context_text, declared_mode_or_none).
    let mut legs: Vec<(String, String, String, Option<String>, Option<String>)> = Vec::new();
    for tc in &day.transit_chains {
        let stops: Vec<&str> = tc.text.split('\u{2192}').collect(); // →
        for w in stops.windows(2) {
            legs.push((
                crate::checks::clean_stop(w[0]),
                crate::checks::clean_stop(w[1]),
                tc.text.clone(),
                None,
                Some(tc.session.clone()),
            ));
        }
    }
    for rl in &day.route_legs {
        legs.push((
            crate::checks::clean_stop(&rl.from),
            crate::checks::clean_stop(&rl.to),
            format!("{} {}", rl.from, rl.to),
            Some(rl.mode.clone()),
            None,
        ));
    }

    for (from, to, ctx, mode, session) in &legs {
        // (2a) MAP-LINK INTEGRITY: every transit/route leg renders a Google Maps
        // link. If a stop cleans to nothing or carries junk that poisons the
        // query, the link is broken. ERROR — must block the gate. (We do NOT try
        // to judge whether the stop is the "right" place: a valid-but-stale name
        // like 牧志 geocodes fine and can't be distinguished from a correct one
        // without false positives. Staleness is prevented at write time and by
        // re-running the review after edits, not by a fuzzy name match here.)
        let raw_stops: Vec<&str> = ctx.split('\u{2192}').collect();
        for (cleaned, idx) in [(from, 0usize), (to, raw_stops.len().saturating_sub(1))] {
            let raw = raw_stops.get(idx).map(|s| s.trim()).unwrap_or("");
            if let Some(reason) = crate::checks::stop_link_problem(cleaned, raw) {
                out.push(Issue {
                    severity: Severity::Error,
                    day: Some(day.day_number),
                    session: session.clone(),
                    message: format!(
                        "Broken map link: stop \"{}\" {} (leg: \"{}\")",
                        truncate(if raw.is_empty() { cleaned } else { raw }, 30),
                        reason,
                        truncate(ctx, 50)
                    ),
                    suggestion: Some(
                        "Use a clean place name as the stop (e.g. \"<車站>\", \"<地標>\") — no parenthetical notes, +步行, or clock times inside the stop itself.".to_string(),
                    ),
                });
            }
        }
        if from.is_empty() || to.is_empty() {
            continue;
        }
        // (2) cross-country leg
        let cf = crate::checks::place_country(from);
        let ct = crate::checks::place_country(to);
        if let (Some(a), Some(b)) = (cf, ct) {
            if a != b {
                out.push(Issue {
                    severity: Severity::Error,
                    day: Some(day.day_number),
                    session: session.clone(),
                    message: format!(
                        "Map leg crosses countries: \"{from}\" ({a}) → \"{to}\" ({b}) — will draw an ocean-spanning route"
                    ),
                    suggestion: Some(
                        "Split into same-country legs, or this leg is a flight (don't render a ground route).".to_string(),
                    ),
                });
            }
        }
        // (3) rail/bus route leg explicitly stored as walking
        if let Some(m) = mode {
            if m == "walking" && crate::checks::mentions_rail_or_bus(ctx) {
                out.push(Issue {
                    severity: Severity::Warning,
                    day: Some(day.day_number),
                    session: session.clone(),
                    message: format!(
                        "Route leg \"{from}\" → \"{to}\" is mode=walking but mentions rail/bus"
                    ),
                    suggestion: Some("Set mode to transit for a rail/bus leg.".to_string()),
                });
            }
        }
        // (4) ambiguous bare-name stop
        for stop in [from, to] {
            if is_ambiguous_stop(stop) {
                out.push(Issue {
                    severity: Severity::Info,
                    day: Some(day.day_number),
                    session: session.clone(),
                    message: format!(
                        "Map stop \"{stop}\" is a bare name without station/city context — may geolocate wrong"
                    ),
                    suggestion: Some(
                        "Add a 駅/站/Station suffix or a city (e.g. \"<車站>駅 <城市>\").".to_string(),
                    ),
                });
            }
        }
    }

    // (5) meal-pin coverage: a lunch/dinner meal with no map pin (｜map:<q> marker
    // or an embedded URL) renders as a plain pill with no "where is it" link.
    for meal in &day.meals {
        if !crate::checks::extract_map_urls(&meal.text).is_empty()
            && !has_meal_map_marker(&meal.text)
        {
            out.push(Issue {
                severity: Severity::Warning,
                day: Some(day.day_number),
                session: Some(meal.session.clone()),
                message: format!(
                    "Meal embeds a raw Google Maps URL and needs short-label rendering: \"{}\"",
                    truncate(&meal.text, 50)
                ),
                suggestion: Some(
                    "Prefer the structured form \"<label>｜map:<place>\" or verify the dashboard build renders meal URLs through render_activity_text.".to_string(),
                ),
            });
        }
        if !crate::checks::meal_has_pin(&meal.text) {
            out.push(Issue {
                severity: Severity::Info,
                day: Some(day.day_number),
                session: Some(meal.session.clone()),
                message: format!(
                    "Meal has no map pin: \"{}\"",
                    truncate(&meal.text, 50)
                ),
                suggestion: Some(
                    "Name a restaurant and add a pin: set-meals … --meal \"<label>｜map:<place>\".".to_string(),
                ),
            });
        }
    }

    // (6) reservation check: a sit-down lunch/dinner restaurant probably needs a
    // booking. Walk-in types (ramen, public-market food courts / 食堂, izakaya
    // street-stall villages, supermarkets, casual soba) can't be reserved, so skip
    // them. Breakfast is also skipped (hotel/conbini, never reserved).
    //
    // BOOKING-AWARE: once the restaurant is enrolled in the booking ledger — i.e.
    // the same session already has an activity with booking_status booked/pending —
    // the reservation is being tracked, so we stop flagging it. This makes the lint
    // self-clearing: set-activity-booking … pending|booked silences it. (Path B:
    // restaurant reservations reuse the activity booking lifecycle.)
    for meal in &day.meals {
        if needs_reservation_check(&meal.session, &meal.text)
            && !session_has_tracked_booking(day, &meal.session)
        {
            out.push(Issue {
                severity: Severity::Info,
                day: Some(day.day_number),
                session: Some(meal.session.clone()),
                message: format!(
                    "Restaurant may need a reservation: \"{}\"",
                    truncate(&meal.text, 50)
                ),
                suggestion: Some(
                    "Enroll it in the booking ledger so it's tracked: add-activity <day> <session> \"<restaurant>\" then set-activity-booking <day> <session> \"<restaurant>\" pending (→ booked once confirmed). Walk-in? Leave as-is.".to_string(),
                ),
            });
        }
    }
}

/// True when the given day+session already has a booking-tracked activity
/// (booking_status booked/pending). Used to suppress the reservation flag once a
/// restaurant has been enrolled in the booking ledger.
fn session_has_tracked_booking(day: &DaySummary, session: &str) -> bool {
    day.activities.iter().any(|a| {
        a.session == session
            && matches!(
                a.booking_status.as_deref(),
                Some("booked") | Some("pending") | Some("waitlist")
            )
    })
}

/// True when a meal is a sit-down lunch/dinner restaurant that plausibly needs a
/// reservation — i.e. NOT a walk-in venue type and NOT breakfast.
///
/// Walk-in types we never flag (you can't / needn't book them):
///   - ramen: 拉麵 / ラーメン / ramen
///   - public-market food courts & 食堂: 市場 / 公設市場 / 食堂 / フードコート / food court
///   - izakaya street-stall villages: 屋台 / 屋台村
///   - supermarkets / convenience: 超市 / スーパー / リウボウ / Ryubo / コンビニ
///   - casual standalone soba shops: そば / 沖繩そば / 沖縄そば (counter-service)
fn needs_reservation_check(session: &str, text: &str) -> bool {
    let s = session.to_lowercase();
    // Only lunch/dinner sit-down meals; breakfast is always walk-in.
    let is_meal_out = s.contains("noon") || s.contains("afternoon") || s.contains("evening") || s.contains("lunch") || s.contains("dinner");
    if !is_meal_out {
        return false;
    }
    let lower = text.to_lowercase();
    // Walk-in markers (any present → no reservation needed).
    const WALK_IN: &[&str] = &[
        "\u{62C9}\u{9EBA}",          // 拉麵
        "\u{30E9}\u{30FC}\u{30E1}\u{30F3}", // ラーメン
        "ramen",
        "\u{5E02}\u{5834}",          // 市場
        "\u{516C}\u{8A2D}\u{5E02}\u{5834}", // 公設市場
        "\u{98DF}\u{5802}",          // 食堂
        "\u{30D5}\u{30FC}\u{30C9}\u{30B3}\u{30FC}\u{30C8}", // フードコート
        "food court",
        "\u{5C4B}\u{53F0}",          // 屋台 (covers 屋台村)
        "\u{8D85}\u{5E02}",          // 超市
        "\u{30B9}\u{30FC}\u{30D1}\u{30FC}", // スーパー
        "\u{30EA}\u{30A6}\u{30DC}\u{30A6}", // リウボウ
        "ryubo",
        "\u{30B3}\u{30F3}\u{30D3}\u{30CB}", // コンビニ
        "\u{305D}\u{3070}",          // そば
        "soba",
    ];
    !WALK_IN.iter().any(|m| lower.contains(m))
}

// A stop is "ambiguous" if it's a short CJK place name with no station/city marker.
fn is_ambiguous_stop(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    // Has an explicit station/place marker → fine.
    let markers = ["\u{99C5}", "\u{7AD9}", "Station", "\u{6A5F}\u{5834}", "Airport", "\u{6A4B}", "\u{516C}\u{5712}", "\u{5BAE}", "\u{5E02}\u{5834}", "\u{901A}"]; // 駅 站 機場 橋 公園 宮 市場 通
    if markers.iter().any(|m| s.contains(m)) {
        return false;
    }
    // Contains a space → likely already "<place> <city>" disambiguated.
    if s.contains(' ') {
        return false;
    }
    // Latin/ASCII name → Maps usually fine.
    if s.is_ascii() {
        return false;
    }
    // Short bare CJK name (≤4 chars) → ambiguous.
    s.chars().count() <= 4
}

/// Renderer-dependent map-text predicate: text with BOTH a newline AND an
/// embedded http(s) URL needs render_activity_text. Newline alone is fine; URL
/// alone is fine. Pure + testable.
fn is_malformed_map_text(text: &str) -> bool {
    let has_newline = text.contains('\n');
    let has_url = text.contains("http://") || text.contains("https://");
    has_newline && has_url
}

fn has_meal_map_marker(text: &str) -> bool {
    let lower = text.to_lowercase();
    lower.contains("\u{FF5C}map:") || lower.contains("|map:")
}

fn truncate(s: &str, n: usize) -> String {
    let flat = s.replace('\n', "\\n");
    if flat.chars().count() <= n {
        flat
    } else {
        let t: String = flat.chars().take(n).collect();
        format!("{t}…")
    }
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
            transit_chains: Vec::new(),
            route_legs: Vec::new(),
            activity_map_urls: Vec::new(),
            meals: Vec::new(),
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
        for u in crate::checks::extract_map_urls(&title) {
            summary.activity_map_urls.push(u);
        }
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

    // Transit chains (transit pills) — prefer ZH text, fall back to EN.
    let mut tod_rows = conn
        .query(
            "SELECT day_number, session_type, \
                    COALESCE(NULLIF(transit_notes_zh, ''), NULLIF(transit_notes, '')) AS transit \
             FROM timesofday WHERE plan_id = ?1 AND destination = ?2",
            params![plan_id.to_string(), dest.to_string()],
        )
        .await
        .map_err(|e| format!("timesofday query failed: {e}"))?;
    while let Some(r) = tod_rows
        .next()
        .await
        .map_err(|e| format!("timesofday row read failed: {e}"))?
    {
        let day_number: i64 = r.get(0).unwrap_or(0);
        let session: String = r.get(1).unwrap_or_default();
        let transit: Option<String> = r.get(2).ok().flatten().filter(|s: &String| !s.is_empty());
        if let (Some(text), Some(summary)) =
            (transit, summaries.iter_mut().find(|d| d.day_number == day_number))
        {
            summary.transit_chains.push(TransitChain { session, text });
        }
    }

    // Route segments.
    let mut seg_rows = conn
        .query(
            "SELECT day_number, from_place, to_place, mode FROM day_route_segments \
             WHERE plan_id = ?1 AND destination = ?2 ORDER BY day_number, sort_order",
            params![plan_id.to_string(), dest.to_string()],
        )
        .await
        .map_err(|e| format!("day_route_segments query failed: {e}"))?;
    while let Some(r) = seg_rows
        .next()
        .await
        .map_err(|e| format!("day_route_segments row read failed: {e}"))?
    {
        let day_number: i64 = r.get(0).unwrap_or(0);
        let from: String = r.get(1).unwrap_or_default();
        let to: String = r.get(2).unwrap_or_default();
        let mode: String = r.get(3).unwrap_or_default();
        if let Some(summary) = summaries.iter_mut().find(|d| d.day_number == day_number) {
            summary.route_legs.push(RouteLeg { from, to, mode });
        }
    }

    // Meals (for pin-coverage check).
    let mut meal_rows = conn
        .query(
            "SELECT day_number, session_type, meal FROM session_meals \
             WHERE plan_id = ?1 AND destination = ?2 ORDER BY day_number, sort_order",
            params![plan_id.to_string(), dest.to_string()],
        )
        .await
        .map_err(|e| format!("session_meals query failed: {e}"))?;
    while let Some(r) = meal_rows
        .next()
        .await
        .map_err(|e| format!("session_meals row read failed: {e}"))?
    {
        let day_number: i64 = r.get(0).unwrap_or(0);
        let session: String = r.get(1).unwrap_or_default();
        let text: String = r.get(2).unwrap_or_default();
        if let Some(summary) = summaries.iter_mut().find(|d| d.day_number == day_number) {
            summary.meals.push(MealEntry { session, text });
        }
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
    crate::cascade::common::resolve_active_destination(conn, plan_id, dest_opt).await
}

#[cfg(test)]
mod map_link_tests {
    use super::*;

    // NOTE: the pure predicates clean_stop / check_stop_linkable / stop_link_problem
    // / place_country / mentions_rail_or_bus / meal_has_pin / extract_map_urls now
    // live in crate::checks and are unit-tested there. The tests below cover the
    // map-link lint BEHAVIOR (validate_map_links wiring) and the predicates that
    // stayed local (is_ambiguous_stop, is_malformed_map_text, needs_reservation_check).

    fn activity(session: &str, title: &str, booking_status: Option<&str>) -> Activity {
        Activity {
            title: title.to_string(),
            session: session.to_string(),
            start_time: None,
            end_time: None,
            duration_min: 0,
            area: None,
            booking_status: booking_status.map(|s| s.to_string()),
            booking_required: booking_status.is_some(),
            book_by: None,
            operating_hours: None,
        }
    }

    fn empty_day() -> DaySummary {
        DaySummary {
            day_number: 1,
            date: "d".into(),
            theme: "x".into(),
            activities: vec![],
            total_duration_min: 0,
            transit_chains: vec![],
            route_legs: vec![],
            activity_map_urls: vec![],
            meals: vec![],
        }
    }

    #[test]
    fn ambiguous_stop_rules() {
        assert!(is_ambiguous_stop("\u{5B89}\u{91CC}")); // 安里
        assert!(!is_ambiguous_stop("\u{5B89}\u{91CC}\u{99C5}")); // 安里駅
        assert!(!is_ambiguous_stop("\u{5B89}\u{91CC}\u{99C5} \u{90A3}\u{8987}"));
        assert!(!is_ambiguous_stop("\u{90A3}\u{8987}\u{6A5F}\u{5834}")); // 那覇機場
        assert!(!is_ambiguous_stop("Kokusai-dori"));
    }

    #[test]
    fn cross_country_leg_flagged() {
        let mut day = empty_day();
        day.date = "2026-06-12".into();
        day.transit_chains = vec![TransitChain {
            session: "morning".into(),
            text: "\u{7D05}\u{6A39}\u{6797}\u{2192}\u{5B89}\u{91CC}".into(),
        }];
        let mut out = Vec::new();
        validate_map_links(&day, &mut out);
        assert!(out.iter().any(|i| {
            matches!(i.severity, Severity::Error) && i.message.contains("crosses countries")
        }));
    }

    #[test]
    fn amp_url_flagged() {
        let mut day = empty_day();
        day.activity_map_urls =
            vec!["https://www.google.com/maps/dir/?api=1&origin=A&destination=B".into()];
        let mut out = Vec::new();
        validate_map_links(&day, &mut out);
        assert!(out.iter().any(|i| i.message.contains("truncated")));
    }

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
        let mut day = empty_day();
        day.day_number = 2;
        day.date = "2026-06-13".into();
        day.theme = "test".into();
        day.activities = vec![activity(
            "evening",
            "晚餐：安里家 — 飯店步行5分\nGoogle Maps：https://www.google.com/maps/search/x",
            None,
        )];
        let mut out = Vec::new();
        validate_map_links(&day, &mut out);
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
        let mut day = empty_day();
        day.date = "2026-06-12".into();
        day.theme = "arrival".into();
        day.activities = vec![
            activity("morning", "Naminoue Shrine", None),
            activity("noon", "Makishi Market", None),
        ];
        let mut out = Vec::new();
        validate_map_links(&day, &mut out);
        assert!(out.is_empty(), "clean activities must not warn, got {} issues", out.len());
    }

    #[test]
    fn lint_accepts_structured_meal_map_marker() {
        let mut day = empty_day();
        day.meals = vec![MealEntry {
            session: "noon".into(),
            text: "午餐：安里屋すば｜map:安里屋すば 那覇 安里".into(),
        }];
        let mut out = Vec::new();
        validate_map_links(&day, &mut out);
        assert!(
            !out.iter().any(|i| i.message.contains("raw Google Maps URL")),
            "structured marker should not warn as raw maps URL: {:?}",
            out.len()
        );
    }

    #[test]
    fn lint_warns_on_raw_maps_url_in_meal() {
        let mut day = empty_day();
        day.meals = vec![MealEntry {
            session: "noon".into(),
            text: "午餐：安里屋すば Google Maps：https://www.google.com/maps/search/asatoya".into(),
        }];
        let mut out = Vec::new();
        validate_map_links(&day, &mut out);
        assert!(
            out.iter()
                .any(|i| matches!(i.severity, Severity::Warning)
                    && i.message.contains("raw Google Maps URL")),
            "raw Google Maps meal URL should warn"
        );
    }

    #[test]
    fn truncate_flattens_newlines_and_caps_length() {
        assert_eq!(truncate("a\nb", 50), "a\\nb");
        let long = "x".repeat(100);
        let t = truncate(&long, 10);
        assert!(t.chars().count() <= 11, "10 chars + ellipsis, got {}", t.chars().count());
        assert!(t.ends_with('…'));
    }

    #[test]
    fn reservation_check_skips_walk_in_and_breakfast() {
        assert!(needs_reservation_check("evening", "\u{665A}\u{9910}\u{FF1A}\u{3086}\u{3046}\u{306A}\u{3093}\u{304E}\u{3044}"));
        assert!(needs_reservation_check("noon", "\u{5348}\u{9910}\u{FF1A}\u{9996}\u{91CC}\u{6BBF}\u{5167}"));
        assert!(!needs_reservation_check("noon", "\u{7B2C}\u{4E00}\u{7261}\u{5FD7}\u{516C}\u{8A2D}\u{5E02}\u{5834} 2F \u{98DF}\u{5802}"));
        assert!(!needs_reservation_check("evening", "\u{6804}\u{753A}\u{5E02}\u{5834} \u{5C4B}\u{53F0}\u{6751}"));
        assert!(!needs_reservation_check("noon", "\u{6CE2}\u{4E4B}\u{4E0A}\u{5468}\u{908A}\u{FF08}\u{6C96}\u{7E69}\u{305D}\u{3070}\u{FF09}"));
        assert!(!needs_reservation_check("noon", "Tonkotsu ramen counter"));
        assert!(!needs_reservation_check("noon", "\u{30EA}\u{30A6}\u{30DC}\u{30A6} \u{5B89}\u{91CC}\u{5E97}"));
        assert!(!needs_reservation_check("morning", "\u{65E9}\u{9910}\u{FF1A}\u{67D0}\u{9910}\u{5EF3}"));
    }

    #[test]
    fn reservation_flag_clears_once_session_has_tracked_booking() {
        let dinner = MealEntry {
            session: "evening".into(),
            text: "\u{665A}\u{9910}\u{FF1A}\u{3086}\u{3046}\u{306A}\u{3093}\u{304E}\u{3044}\u{FF5C}map:x".into(),
        };
        let mut base = empty_day();
        base.meals = vec![dinner];
        let mut out = Vec::new();
        validate_map_links(&base, &mut out);
        assert!(
            out.iter().any(|i| i.message.starts_with("Restaurant may need a reservation")),
            "unbooked sit-down restaurant should be flagged"
        );

        let mut booked = DaySummary {
            activities: vec![activity("evening", "restaurant", Some("pending"))],
            ..base
        };
        booked.meals = vec![MealEntry {
            session: "evening".into(),
            text: "\u{665A}\u{9910}\u{FF1A}\u{3086}\u{3046}\u{306A}\u{3093}\u{304E}\u{3044}\u{FF5C}map:x".into(),
        }];
        let mut out2 = Vec::new();
        validate_map_links(&booked, &mut out2);
        assert!(
            !out2.iter().any(|i| i.message.starts_with("Restaurant may need a reservation")),
            "a pending booking in the session should clear the reservation flag"
        );

        assert!(session_has_tracked_booking(&booked, "evening"));
        assert!(!session_has_tracked_booking(&booked, "noon"));
    }

    #[test]
    fn clean_same_country_leg_ok() {
        let mut day = empty_day();
        day.transit_chains = vec![TransitChain {
            session: "afternoon".into(),
            text: "\u{90A3}\u{8987}\u{6A5F}\u{5834}\u{2192}\u{5B89}\u{91CC}\u{99C5} \u{90A3}\u{8987}".into(),
        }];
        let mut out = Vec::new();
        validate_map_links(&day, &mut out);
        assert!(!out.iter().any(|i| matches!(i.severity, Severity::Error)));
    }
}
