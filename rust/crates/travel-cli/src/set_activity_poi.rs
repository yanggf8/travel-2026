// `travel set-activity-poi <day> <session> <poi_id> [--match "<title substring>"]`
// — link an itinerary activity to a destination POI by id, so the dashboard
// attaches the POI's ticket price + map pin via a durable FK instead of a
// fragile exact-title match. NO CASCADE.
//
// Mirrors the neighboring set-activity-* commands:
//   1. Resolve the target activity by (plan_id, destination, day, session).
//      If that (day, session) holds >1 activity, `--match <substring>`
//      disambiguates against a case-insensitive title substring. Zero OR >1
//      matches without a unique disambiguator → FAIL LOUD (non-zero, no write).
//   2. Validate <poi_id> EXISTS in destination_pois for the plan's dest slug.
//      Unknown poi_id → FAIL LOUD.
//   3. UPDATE activities SET poi_id = ? … ; require rows_affected == 1.
//   4. Audit triad: operation_runs + plan_events (dest_process + timeline)
//      + plan_event_data KV + plans.version bump — same as set-activity-title.

use libsql::Connection;
use travel_db::repo::itinerary;

#[derive(Default, Debug)]
struct ParsedArgs {
    auto: bool,
    day: i64,
    session: String,
    poi_id: String,
    match_substr: Option<String>,
    dest: Option<String>,
}

#[derive(Debug, Clone)]
struct AutoActivity {
    id: String,
    day: i64,
    session: String,
    // Selected only to make the scan order (ORDER BY day_number, session_type,
    // sort_order, id) explicit + deterministic; not read back in Rust.
    #[allow(dead_code)]
    sort_order: i64,
    title: String,
}

// pub(crate): reused by set_activity.rs's post-add POI hint (the ONE match rule).
#[derive(Debug, Clone)]
pub(crate) struct AutoPoi {
    pub(crate) poi_id: String,
    pub(crate) title: String,
    pub(crate) geocoded: bool,
}

#[derive(Debug, Clone)]
struct AutoLinked {
    activity: AutoActivity,
    poi_id: String,
}

#[derive(Debug, Clone)]
struct AutoUnlinked {
    activity: AutoActivity,
    reason: String,
}

#[derive(Debug, Clone)]
struct AutoResult {
    linked: Vec<AutoLinked>,
    unlinked: Vec<AutoUnlinked>,
    version_before: Option<i64>,
    version_after: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AutoMatch {
    Link(String),
    Unlinked(String),
}

/// CLI entry: `travel set-activity-poi <day> <session> <poi_id> [--match "<sub>"] [--dest <slug>]`.
pub async fn run(args: &[String], plan_id: String) -> Result<(), String> {
    let parsed = parse_args(args)?;

    let conn = match crate::db::connect_write().await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error: failed to connect to Turso (write tier): {e}");
            std::process::exit(1);
        }
    };

    let destination = match read_destination(&conn, &plan_id, &parsed.dest).await {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    };

    if parsed.auto {
        match execute_auto(&conn, &plan_id, &destination).await {
            Ok(result) => {
                print_auto_result(&destination, &result);
                return Ok(());
            }
            Err(e) => {
                eprintln!("Error: set-activity-poi --auto failed: {e}");
                std::process::exit(1);
            }
        }
    }

    match execute(&conn, &plan_id, &destination, &parsed).await {
        Ok(title) => {
            println!("\n📍 Linking activity to POI:");
            println!("   Destination: {destination}");
            println!("   Day {} {}: \"{}\"", parsed.day, parsed.session, title);
            println!("   POI id: {}", parsed.poi_id);
            println!("✅ Activity linked to POI");
            Ok(())
        }
        Err(e) => {
            eprintln!("Error: set-activity-poi failed: {e}");
            std::process::exit(1);
        }
    }
}

fn parse_args(args: &[String]) -> Result<ParsedArgs, String> {
    let mut p = ParsedArgs::default();
    let mut positional: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        match a.as_str() {
            "--auto" => {
                p.auto = true;
                i += 1;
            }
            "--match" => {
                p.match_substr = Some(arg_value(args, i, "--match")?);
                i += 2;
            }
            "--dest" => {
                p.dest = Some(arg_value(args, i, "--dest")?);
                i += 2;
            }
            // Accept-and-skip plan-selection flags (plan resolution is done in main.rs via
            // plan_resolver::resolve_plan_id, matching the neighboring set-* mutations;
            // the resolved plan_id is passed into run()).
            f if crate::plan_resolver::is_resolver_flag(f) => {
                let _ = arg_value(args, i, f)?;
                i += 2;
            }
            other if other.starts_with("--") => {
                return Err(format!("unknown argument: {other}"));
            }
            _ => {
                positional.push(a.clone());
                i += 1;
            }
        }
    }
    if p.auto {
        if !positional.is_empty() {
            return Err(
                "--auto cannot be combined with <day> <session> <poi_id>; use either --auto or a single manual link"
                    .to_string(),
            );
        }
        if p.match_substr.is_some() {
            return Err("--auto cannot be combined with --match".to_string());
        }
        return Ok(p);
    }

    if positional.len() < 3 {
        return Err(usage_error());
    }
    p.day = positional[0]
        .parse::<i64>()
        .map_err(|_| "<day> must be a positive integer".to_string())?;
    if p.day < 1 {
        return Err("<day> must be a positive integer".to_string());
    }
    p.session = positional[1].clone();
    if !["morning", "noon", "afternoon", "evening"].contains(&p.session.as_str()) {
        return Err("<session> must be one of: morning|noon|afternoon|evening".to_string());
    }
    p.poi_id = positional[2].clone();
    if p.poi_id.trim().is_empty() {
        return Err("<poi_id> cannot be empty".to_string());
    }
    Ok(p)
}

fn usage_error() -> String {
    "Usage: set-activity-poi <day> <session> <poi_id> [--match \"<title substring>\"] [--dest <slug>]
       set-activity-poi --auto [--dest <slug>]
Example: set-activity-poi 2 morning shuri_castle --match \"Shurijo\"
Example: set-activity-poi --auto --dest osaka_kyoto_2026"
        .to_string()
}

fn arg_value(args: &[String], i: usize, flag: &str) -> Result<String, String> {
    args.get(i + 1)
        .cloned()
        .ok_or_else(|| format!("missing value for {flag}"))
}

async fn read_destination(
    conn: &Connection,
    plan_id: &str,
    dest_opt: &Option<String>,
) -> Result<String, String> {
    crate::cascade::common::resolve_active_destination(conn, plan_id, dest_opt.as_deref()).await
}

/// Resolve the unique target activity (id, title) in (day, session), narrowed
/// by an optional case-insensitive title substring. Fails loud on zero or >1.
async fn resolve_activity(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    session: &str,
    match_substr: &Option<String>,
) -> Result<(String, String), String> {
    let mut rows = conn
        .query(
            "SELECT id, title FROM activities \
             WHERE plan_id = ?1 AND destination = ?2 AND day_number = ?3 \
               AND session_type = ?4 ORDER BY sort_order",
            libsql::params![
                plan_id.to_string(),
                destination.to_string(),
                day,
                session.to_string()
            ],
        )
        .await
        .map_err(|e| format!("activities list query failed: {e}"))?;

    let needle = match_substr.as_ref().map(|s| s.to_lowercase());
    let mut matches: Vec<(String, String)> = Vec::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("activities list row read failed: {e}"))?
    {
        let id: String = row.get(0).map_err(|e| format!("id col read failed: {e}"))?;
        let title: Option<String> = row.get(1).ok();
        let t = title.as_deref().unwrap_or("");
        match &needle {
            Some(n) if !t.to_lowercase().contains(n) => continue,
            _ => matches.push((id, t.to_string())),
        }
    }

    match matches.len() {
        0 => Err(format!(
            "no activity found in Day {day} {session}{}",
            match_substr
                .as_ref()
                .map(|s| format!(" matching \"{s}\""))
                .unwrap_or_default()
        )),
        1 => Ok(matches.into_iter().next().unwrap()),
        n => Err(format!(
            "{n} activities match Day {day} {session}{} — disambiguate with --match \"<title substring>\"",
            match_substr
                .as_ref()
                .map(|s| format!(" \"{s}\""))
                .unwrap_or_default()
        )),
    }
}

/// Fail loud unless `poi_id` exists in destination_pois for this dest slug.
async fn assert_poi_exists(
    conn: &Connection,
    destination: &str,
    poi_id: &str,
) -> Result<(), String> {
    if itinerary::poi_exists(conn, destination, poi_id).await? {
        return Ok(());
    }
    Err(format!(
        "unknown poi_id \"{poi_id}\" for destination \"{destination}\" \
         (no row in destination_pois)"
    ))
}

fn is_trailing_gloss_char(c: char) -> bool {
    c.is_whitespace()
        || matches!(
            c,
            '\u{4E00}'..='\u{9FFF}'
                | '\u{3040}'..='\u{309F}'
                | '\u{30A0}'..='\u{30FF}'
                | '\u{3000}'..='\u{303F}'
                | '\u{FF00}'..='\u{FFEF}'
        )
}

fn strip_trailing_zh_gloss(title: &str) -> String {
    let trimmed = title.trim_end();
    let mut cut = trimmed.len();
    let mut saw_script_or_fullwidth = false;

    for (idx, ch) in trimmed.char_indices().rev() {
        if is_trailing_gloss_char(ch) {
            if !ch.is_whitespace() {
                saw_script_or_fullwidth = true;
            }
            cut = idx;
            continue;
        }
        break;
    }

    if saw_script_or_fullwidth {
        trimmed[..cut].trim_end().to_string()
    } else {
        trimmed.to_string()
    }
}

pub(crate) fn resolve_auto_match(activity_title: &str, pois: &[AutoPoi]) -> AutoMatch {
    let normalized = strip_trailing_zh_gloss(activity_title).to_lowercase();
    let exact: Vec<&AutoPoi> = pois
        .iter()
        .filter(|p| p.title.to_lowercase() == normalized)
        .collect();
    let matches = if exact.is_empty() {
        pois.iter()
            .filter(|p| {
                let title = p.title.to_lowercase();
                !normalized.is_empty()
                    && !title.is_empty()
                    && (normalized.contains(&title) || title.contains(&normalized))
            })
            .collect::<Vec<&AutoPoi>>()
    } else {
        exact
    };

    if matches.is_empty() {
        return AutoMatch::Unlinked("no POI match".to_string());
    }

    let geocoded: Vec<&AutoPoi> = matches.iter().copied().filter(|p| p.geocoded).collect();
    match geocoded.len() {
        0 => AutoMatch::Unlinked("matched but POI ungeocoded".to_string()),
        1 => AutoMatch::Link(geocoded[0].poi_id.clone()),
        _ => AutoMatch::Unlinked("ambiguous POI match".to_string()),
    }
}

async fn list_null_poi_activities(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
) -> Result<Vec<AutoActivity>, String> {
    let mut rows = conn
        .query(
            "SELECT id, day_number, session_type, sort_order, title \
             FROM activities \
             WHERE plan_id = ?1 AND destination = ?2 AND poi_id IS NULL \
             ORDER BY day_number, session_type, sort_order, id",
            libsql::params![plan_id.to_string(), destination.to_string()],
        )
        .await
        .map_err(|e| format!("NULL-poi activities query failed: {e}"))?;
    let mut out = Vec::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("NULL-poi activities row read failed: {e}"))?
    {
        out.push(AutoActivity {
            id: row.get(0).map_err(|e| format!("activity id col read failed: {e}"))?,
            day: row.get(1).map_err(|e| format!("activity day col read failed: {e}"))?,
            session: row.get(2).map_err(|e| format!("activity session col read failed: {e}"))?,
            sort_order: row.get(3).map_err(|e| format!("activity sort_order col read failed: {e}"))?,
            title: row.get(4).map_err(|e| format!("activity title col read failed: {e}"))?,
        });
    }
    Ok(out)
}

pub(crate) async fn list_destination_pois_for_auto(
    conn: &Connection,
    destination: &str,
) -> Result<Vec<AutoPoi>, String> {
    let mut rows = conn
        .query(
            "SELECT poi_id, COALESCE(title, ''), \
                    CASE WHEN lat IS NOT NULL AND lon IS NOT NULL \
                           AND TRIM(CAST(lat AS TEXT)) <> '' \
                           AND TRIM(CAST(lon AS TEXT)) <> '' \
                         THEN 1 ELSE 0 END AS geocoded \
             FROM destination_pois \
             WHERE slug = ?1 \
             ORDER BY poi_id",
            libsql::params![destination.to_string()],
        )
        .await
        .map_err(|e| format!("destination_pois auto query failed: {e}"))?;
    let mut out = Vec::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("destination_pois auto row read failed: {e}"))?
    {
        let geocoded: i64 = row.get(2).unwrap_or(0);
        out.push(AutoPoi {
            poi_id: row.get(0).map_err(|e| format!("poi_id col read failed: {e}"))?,
            title: row.get(1).unwrap_or_default(),
            geocoded: geocoded == 1,
        });
    }
    Ok(out)
}

async fn execute_auto(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
) -> Result<AutoResult, String> {
    let activities = list_null_poi_activities(conn, plan_id, destination).await?;
    if activities.is_empty() {
        return Ok(AutoResult {
            linked: Vec::new(),
            unlinked: Vec::new(),
            version_before: None,
            version_after: None,
        });
    }

    let pois = list_destination_pois_for_auto(conn, destination).await?;
    let mut linked = Vec::new();
    let mut unlinked = Vec::new();

    for activity in activities {
        match resolve_auto_match(&activity.title, &pois) {
            AutoMatch::Link(poi_id) => linked.push(AutoLinked { activity, poi_id }),
            AutoMatch::Unlinked(reason) => unlinked.push(AutoUnlinked { activity, reason }),
        }
    }

    if linked.is_empty() {
        return Ok(AutoResult {
            linked,
            unlinked,
            version_before: None,
            version_after: None,
        });
    }

    let now_iso = now_rfc3339();
    let now_db = now_db_datetime();
    let version_before = read_version(conn, plan_id).await?;
    let version_after = version_before + 1;
    let mut touched_days: Vec<i64> = Vec::new();

    let mut dest_process_so =
        next_dest_process_sort_order(conn, plan_id, destination, "process_5_daily_itinerary").await?;
    let mut timeline_so = next_timeline_sort_order(conn, plan_id).await?;

    for link in &linked {
        let affected = itinerary::set_activity_poi(
            conn,
            plan_id,
            destination,
            link.activity.day,
            &link.activity.session,
            &link.activity.id,
            &link.poi_id,
            &now_db,
        )
        .await?;
        if affected != 1 {
            return Err(format!(
                "activities UPDATE affected {affected} rows (expected 1) for id={}",
                link.activity.id
            ));
        }

        if !touched_days.contains(&link.activity.day) {
            touch_day(conn, plan_id, destination, link.activity.day).await?;
            touched_days.push(link.activity.day);
        }

        let kv: Vec<(&str, String)> = vec![
            ("day_number", link.activity.day.to_string()),
            ("session", link.activity.session.clone()),
            ("activity_id", link.activity.id.clone()),
            ("title", link.activity.title.clone()),
            ("poi_id", link.poi_id.clone()),
            ("mode", "auto".to_string()),
        ];
        insert_event(
            conn,
            plan_id,
            "dest_process",
            destination,
            "process_5_daily_itinerary",
            dest_process_so,
            "activity_poi_linked",
            &now_iso,
        )
        .await?;
        insert_kv(
            conn,
            plan_id,
            "dest_process",
            destination,
            "process_5_daily_itinerary",
            dest_process_so,
            &kv,
        )
        .await?;
        dest_process_so += 1;

        insert_event(
            conn,
            plan_id,
            "timeline",
            "",
            "process_5_daily_itinerary",
            timeline_so,
            "activity_poi_linked",
            &now_iso,
        )
        .await?;
        insert_kv(
            conn,
            plan_id,
            "timeline",
            "",
            "process_5_daily_itinerary",
            timeline_so,
            &kv,
        )
        .await?;
        timeline_so += 1;
    }

    crate::cascade::common::record_operation(
        conn,
        plan_id,
        "set-activity-poi",
        &format!("auto-linked {} POI(s)", linked.len()),
        version_before,
        version_after,
        &now_db,
    )
    .await?;

    Ok(AutoResult {
        linked,
        unlinked,
        version_before: Some(version_before),
        version_after: Some(version_after),
    })
}

fn print_auto_result(destination: &str, result: &AutoResult) {
    println!("\n📍 set-activity-poi --auto ({destination})");
    if result.linked.is_empty() && result.unlinked.is_empty() {
        println!("nothing to link");
        return;
    }

    println!("✅ linked {}:", result.linked.len());
    for link in &result.linked {
        println!(
            "   D{} {:<9} \"{}\" -> {}",
            link.activity.day, link.activity.session, link.activity.title, link.poi_id
        );
    }

    if !result.unlinked.is_empty() {
        println!(
            "⚠ unlinked {} (link manually with set-activity-poi <day> <session> <poi_id> --match \"<title substring>\"):",
            result.unlinked.len()
        );
        for item in &result.unlinked {
            println!(
                "   D{} {:<9} \"{}\" - {}",
                item.activity.day, item.activity.session, item.activity.title, item.reason
            );
        }
    }

    if let (Some(before), Some(after)) = (result.version_before, result.version_after) {
        println!(
            "Version {before} -> {after} (auto-linked {} POI(s)).",
            result.linked.len()
        );
    }
}

async fn execute(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    parsed: &ParsedArgs,
) -> Result<String, String> {
    // 1. Resolve the unique target activity (fail loud on 0 / >1).
    let (activity_id, title) = resolve_activity(
        conn,
        plan_id,
        destination,
        parsed.day,
        &parsed.session,
        &parsed.match_substr,
    )
    .await?;

    // 2. Validate the poi_id exists (fail loud before any write).
    assert_poi_exists(conn, destination, &parsed.poi_id).await?;

    let now_iso = now_rfc3339();
    let now_db = now_db_datetime();
    let version_before = read_version(conn, plan_id).await?;
    let version_after = version_before + 1;

    // 3. UPDATE activities.poi_id — require rows_affected == 1.
    let affected = itinerary::set_activity_poi(
        conn,
        plan_id,
        destination,
        parsed.day,
        &parsed.session,
        &activity_id,
        &parsed.poi_id,
        &now_db,
    )
    .await?;
    if affected != 1 {
        return Err(format!(
            "activities UPDATE affected {affected} rows (expected 1) for id={activity_id}"
        ));
    }
    touch_day(conn, plan_id, destination, parsed.day).await?;

    // 4. Audit triad — plan_events (dest_process + timeline) + KV.
    let kv: Vec<(&str, String)> = vec![
        ("day_number", parsed.day.to_string()),
        ("session", parsed.session.clone()),
        ("activity_id", activity_id.clone()),
        ("title", title.clone()),
        ("poi_id", parsed.poi_id.clone()),
    ];

    let dest_process_so =
        next_dest_process_sort_order(conn, plan_id, destination, "process_5_daily_itinerary").await?;
    let timeline_so = next_timeline_sort_order(conn, plan_id).await?;
    insert_event(
        conn,
        plan_id,
        "dest_process",
        destination,
        "process_5_daily_itinerary",
        dest_process_so,
        "activity_poi_linked",
        &now_iso,
    )
    .await?;
    insert_kv(
        conn,
        plan_id,
        "dest_process",
        destination,
        "process_5_daily_itinerary",
        dest_process_so,
        &kv,
    )
    .await?;
    insert_event(
        conn,
        plan_id,
        "timeline",
        "",
        "process_5_daily_itinerary",
        timeline_so,
        "activity_poi_linked",
        &now_iso,
    )
    .await?;
    insert_kv(
        conn,
        plan_id,
        "timeline",
        "",
        "process_5_daily_itinerary",
        timeline_so,
        &kv,
    )
    .await?;

    // operation_runs audit row + plans.version bump (audit-triad back half).
    crate::cascade::common::record_operation(
        conn,
        plan_id,
        "set-activity-poi",
        &format!("D{} {} → {}", parsed.day, parsed.session, parsed.poi_id),
        version_before,
        version_after,
        &now_db,
    )
    .await?;

    Ok(title)
}

async fn touch_day(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
) -> Result<(), String> {
    conn.execute(
        "UPDATE days SET updated_at = ?1 WHERE plan_id = ?2 AND destination = ?3 AND day_number = ?4",
        libsql::params![now_db_datetime(), plan_id.to_string(), destination.to_string(), day],
    )
    .await
    .map_err(|e| format!("days touch UPDATE failed: {e}"))?;
    Ok(())
}

async fn read_version(conn: &Connection, plan_id: &str) -> Result<i64, String> {
    let mut rows = conn
        .query(
            "SELECT version FROM plans WHERE plan_id = ?1",
            libsql::params![plan_id.to_string()],
        )
        .await
        .map_err(|e| format!("plans query failed: {e}"))?;
    if let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("plans row read failed: {e}"))?
    {
        let v: i64 = row.get(0).map_err(|e| format!("version col read failed: {e}"))?;
        return Ok(v);
    }
    Err(format!("plans row missing for plan_id={plan_id}"))
}

async fn next_dest_process_sort_order(
    conn: &Connection,
    plan_id: &str,
    dest: &str,
    process_id: &str,
) -> Result<i64, String> {
    let mut rows = conn
        .query(
            "SELECT COALESCE(MAX(sort_order), -1) AS m FROM plan_events \
             WHERE plan_id = ?1 AND scope = 'dest_process' \
               AND destination = ?2 AND process_id = ?3",
            libsql::params![plan_id.to_string(), dest.to_string(), process_id.to_string()],
        )
        .await
        .map_err(|e| format!("plan_events MAX query failed: {e}"))?;
    if let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("plan_events MAX row read failed: {e}"))?
    {
        let m: i64 = row.get(0).map_err(|e| format!("plan_events MAX col read failed: {e}"))?;
        return Ok(m + 1);
    }
    Ok(0)
}

async fn next_timeline_sort_order(conn: &Connection, plan_id: &str) -> Result<i64, String> {
    let mut rows = conn
        .query(
            "SELECT COALESCE(MAX(sort_order), -1) AS m FROM plan_events \
             WHERE plan_id = ?1 AND scope = 'timeline'",
            libsql::params![plan_id.to_string()],
        )
        .await
        .map_err(|e| format!("plan_events MAX(timeline) query failed: {e}"))?;
    if let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("plan_events MAX(timeline) row read failed: {e}"))?
    {
        let m: i64 = row
            .get(0)
            .map_err(|e| format!("plan_events MAX(timeline) col read failed: {e}"))?;
        return Ok(m + 1);
    }
    Ok(0)
}

#[allow(clippy::too_many_arguments)]
async fn insert_event(
    conn: &Connection,
    plan_id: &str,
    scope: &str,
    destination: &str,
    process_id: &str,
    sort_order: i64,
    event: &str,
    event_at: &str,
) -> Result<(), String> {
    conn.execute(
        "DELETE FROM plan_events \
         WHERE plan_id = ?1 AND scope = ?2 AND destination = ?3 \
           AND process_id = ?4 AND sort_order = ?5",
        libsql::params![plan_id.to_string(), scope.to_string(), destination.to_string(), process_id.to_string(), sort_order],
    )
    .await
    .map_err(|e| format!("plan_events DELETE failed: {e}"))?;
    conn.execute(
        "INSERT INTO plan_events \
            (plan_id, scope, destination, process_id, sort_order, \
             event, event_at, from_state, to_state) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, NULL)",
        libsql::params![plan_id.to_string(), scope.to_string(), destination.to_string(), process_id.to_string(), sort_order, event.to_string(), event_at.to_string()],
    )
    .await
    .map_err(|e| format!("plan_events INSERT failed: {e}"))?;
    Ok(())
}

async fn insert_kv(
    conn: &Connection,
    plan_id: &str,
    scope: &str,
    destination: &str,
    process_id: &str,
    sort_order: i64,
    kv: &[(&str, String)],
) -> Result<(), String> {
    conn.execute(
        "DELETE FROM plan_event_data \
         WHERE plan_id = ?1 AND scope = ?2 AND destination = ?3 \
           AND process_id = ?4 AND sort_order = ?5",
        libsql::params![plan_id.to_string(), scope.to_string(), destination.to_string(), process_id.to_string(), sort_order],
    )
    .await
    .map_err(|e| format!("plan_event_data DELETE failed: {e}"))?;
    for (k, v) in kv {
        conn.execute(
            "INSERT INTO plan_event_data \
                (plan_id, scope, destination, process_id, sort_order, key, value) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            libsql::params![plan_id.to_string(), scope.to_string(), destination.to_string(), process_id.to_string(), sort_order, k.to_string(), v.clone()],
        )
        .await
        .map_err(|e| format!("plan_event_data INSERT failed: {e}"))?;
    }
    Ok(())
}

fn now_rfc3339() -> String {
    let (year, month, day, hour, min, sec, ms) = now_civil();
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{min:02}:{sec:02}.{ms:03}Z")
}

fn now_db_datetime() -> String {
    let (year, month, day, hour, min, sec, _) = now_civil();
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{min:02}:{sec:02}")
}

fn now_civil() -> (i32, u32, u32, u32, u32, u32, u32) {
    use std::time::{SystemTime, UNIX_EPOCH};
    let d = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = d.as_secs() as i64;
    let ms = d.subsec_millis();
    let days = secs.div_euclid(86_400);
    let sod = secs.rem_euclid(86_400) as u32;
    let hour = sod / 3600;
    let min = (sod % 3600) / 60;
    let sec = sod % 60;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d_ = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m as u32, d_ as u32, hour, min, sec, ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_args_minimal() {
        let p = parse_args(&[
            "2".to_string(),
            "morning".to_string(),
            "shuri_castle".to_string(),
        ])
        .unwrap();
        assert_eq!(p.day, 2);
        assert_eq!(p.session, "morning");
        assert_eq!(p.poi_id, "shuri_castle");
        assert!(p.match_substr.is_none());
    }

    #[test]
    fn parse_args_with_match_and_dest() {
        let p = parse_args(&[
            "2".to_string(),
            "morning".to_string(),
            "shuri_castle".to_string(),
            "--match".to_string(),
            "Shurijo".to_string(),
            "--dest".to_string(),
            "okinawa_2026".to_string(),
        ])
        .unwrap();
        assert_eq!(p.match_substr.as_deref(), Some("Shurijo"));
        assert_eq!(p.dest.as_deref(), Some("okinawa_2026"));
    }

    #[test]
    fn parse_args_accepts_and_skips_plan_id() {
        let p = parse_args(&[
            "1".to_string(),
            "afternoon".to_string(),
            "kokusaidori".to_string(),
            "--plan-id".to_string(),
            "okinawa-2026".to_string(),
        ])
        .unwrap();
        assert_eq!(p.day, 1);
        assert_eq!(p.poi_id, "kokusaidori");
    }

    #[test]
    fn parse_args_rejects_bad_session() {
        assert!(parse_args(&[
            "2".to_string(),
            "lunch".to_string(),
            "x".to_string()
        ])
        .is_err());
    }

    #[test]
    fn parse_args_rejects_missing_positional() {
        assert!(parse_args(&["2".to_string(), "morning".to_string()]).is_err());
    }

    #[test]
    fn parse_args_rejects_bad_day() {
        assert!(parse_args(&[
            "0".to_string(),
            "morning".to_string(),
            "x".to_string()
        ])
        .is_err());
    }

    #[test]
    fn parse_args_accepts_auto_with_dest() {
        let p = parse_args(&[
            "--auto".to_string(),
            "--dest".to_string(),
            "osaka_kyoto_2026".to_string(),
        ])
        .unwrap();
        assert!(p.auto);
        assert_eq!(p.dest.as_deref(), Some("osaka_kyoto_2026"));
    }

    #[test]
    fn parse_args_rejects_auto_with_positionals() {
        let err = parse_args(&[
            "--auto".to_string(),
            "1".to_string(),
            "morning".to_string(),
            "foo".to_string(),
        ])
        .unwrap_err();
        assert!(err.contains("--auto cannot be combined with <day> <session> <poi_id>"));
    }

    #[test]
    fn strip_trailing_zh_gloss_removes_only_trailing_gloss() {
        assert_eq!(strip_trailing_zh_gloss("Dotonbori 道頓堀"), "Dotonbori");
        assert_eq!(
            strip_trailing_zh_gloss("Kuromon Ichiba Market 黑門市場"),
            "Kuromon Ichiba Market"
        );
        assert_eq!(
            strip_trailing_zh_gloss("Umeda Sky Building 梅田スカイビル"),
            "Umeda Sky Building"
        );
        assert_eq!(strip_trailing_zh_gloss("Namba Parks"), "Namba Parks");
    }

    #[test]
    fn resolve_auto_match_exact_substring_ambiguous_and_ungeocoded() {
        let pois = vec![
            AutoPoi {
                poi_id: "kuromon".to_string(),
                title: "Kuromon Ichiba Market".to_string(),
                geocoded: true,
            },
            AutoPoi {
                poi_id: "dotonbori".to_string(),
                title: "Dotonbori Canal".to_string(),
                geocoded: true,
            },
            AutoPoi {
                poi_id: "namba1".to_string(),
                title: "Namba Parks".to_string(),
                geocoded: true,
            },
            AutoPoi {
                poi_id: "namba2".to_string(),
                title: "Namba Yasaka Shrine".to_string(),
                geocoded: true,
            },
            AutoPoi {
                poi_id: "tower".to_string(),
                title: "Tsutenkaku Tower".to_string(),
                geocoded: false,
            },
        ];
        assert_eq!(
            resolve_auto_match("Kuromon Ichiba Market 黑門市場", &pois),
            AutoMatch::Link("kuromon".to_string())
        );
        assert_eq!(
            resolve_auto_match("Dotonbori 道頓堀", &pois),
            AutoMatch::Link("dotonbori".to_string())
        );
        assert_eq!(
            resolve_auto_match("Namba", &pois),
            AutoMatch::Unlinked("ambiguous POI match".to_string())
        );
        assert_eq!(
            resolve_auto_match("No Such Place", &pois),
            AutoMatch::Unlinked("no POI match".to_string())
        );
        assert_eq!(
            resolve_auto_match("Tsutenkaku Tower 通天閣", &pois),
            AutoMatch::Unlinked("matched but POI ungeocoded".to_string())
        );
    }
}
