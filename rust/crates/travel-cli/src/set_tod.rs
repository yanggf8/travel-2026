// `travel set-tod-focus`, `travel set-tod-time-range`, `travel set-tod-zh`
// (and their `set-session-*` aliases) — port of
// src/cli/commands/session.ts. NO CASCADE.
//
// All three commands write to the `timesofday` row for the given
// (plan, dest, day, session), touch the affected day, and emit
// plan_events (1 dest_process + 1 timeline) with the same KV
// payload the TS spread would produce. set-tod-zh additionally
// writes the child rows in `session_activities_zh` and
// `session_meals`.

use libsql::Connection;

#[derive(Default, Debug)]
struct ParsedFocus {
    day: i64,
    session: String,
    focus: Option<String>,
    focus_zh: Option<String>,
    dest: Option<String>,
}

#[derive(Default, Debug)]
struct ParsedTimeRange {
    day: i64,
    session: String,
    start: Option<String>,
    end: Option<String>,
    dest: Option<String>,
}

#[derive(Default, Debug)]
struct ParsedZh {
    day: i64,
    session: String,
    focus_zh: Option<String>,
    transit_zh: Option<String>,
    activities_zh: Option<Vec<String>>,
    /// `--clear-activities`: remove ALL session_activities_zh rows for this
    /// session (the legacy worker then falls back to nothing; the -rs worker
    /// already renders English titles). Avoids re-passing the whole list or
    /// dropping to raw `db exec DELETE`.
    clear_activities: bool,
    dest: Option<String>,
}

// ─────────────────────────────────────────────────────────────────
// set-tod-focus / set-session-focus
// ─────────────────────────────────────────────────────────────────

pub async fn run_focus(
    args: &[String],
    plan_id: String,
) -> Result<(), String> {
    let parsed = parse_focus(args)?;
    let conn = crate::db::connect_write().await?;
    let destination = match read_destination(&conn, &plan_id, &parsed.dest).await {
        Ok(d) => d,
        Err(e) => return Err(e),
    };
    require_session(&conn, &plan_id, &destination, parsed.day, &parsed.session).await?;
    let focus = parsed.focus.as_deref();
    let focus_zh = parsed.focus_zh.as_deref();

    println!(
        "✅ Focus set for D{} {}: \"{}\"",
        parsed.day,
        parsed.session,
        focus.unwrap_or("(cleared)")
    );

    let kv: Vec<(&str, String)> = vec![
        ("day_number", parsed.day.to_string()),
        ("session", parsed.session.clone()),
        (
            "focus",
            focus.map(str::to_string).unwrap_or_else(|| "null".to_string()),
        ),
        // focus_zh is ALWAYS emitted (per the TS spread — even when
        // undefined, it becomes the literal string "undefined" via
        // eventDataToKv).
        (
            "focus_zh",
            focus_zh
                .map(str::to_string)
                .unwrap_or_else(|| "undefined".to_string()),
        ),
    ];

    execute_tod(
        &conn,
        &plan_id,
        &destination,
        parsed.day,
        &parsed.session,
        focus_zh,
        None, // focus_zh: passed above
        None,
        None,
        &kv,
        "itinerary_session_focus_set",
        "set-tod-focus",
        &format!("D{}/{}", parsed.day, parsed.session),
    )
    .await?;

    if let Some(f) = focus {
        // Apply the focus column.
        apply_tod_field(
            &conn,
            &plan_id,
            &destination,
            parsed.day,
            &parsed.session,
            "focus",
            Some(f.to_string()),
        )
        .await?;
    }
    if let Some(fz) = focus_zh {
        apply_tod_field(
            &conn,
            &plan_id,
            &destination,
            parsed.day,
            &parsed.session,
            "focus_zh",
            Some(fz.to_string()),
        )
        .await?;
    }
    // Dashboard renders ZH by default: an English-only focus edit won't show
    // until focus_zh is also updated. Warn if a stale focus_zh exists.
    if focus.is_some() && focus_zh.is_none() {
        let zh = read_tod_zh_field(&conn, &plan_id, &destination, parsed.day, &parsed.session, "focus_zh")
            .await
            .unwrap_or_default();
        if !zh.trim().is_empty() {
            crate::checks::warn_zh_stale(
                "focus",
                &format!(
                    "set-tod-zh {} {} --zh \"<chinese focus>\" --dest {destination}",
                    parsed.day, parsed.session
                ),
            );
        }
    }
    Ok(())
}

/// Read one `timesofday` ZH column (empty string if NULL/missing), to decide
/// whether to warn about a stale default-ZH dashboard render.
async fn read_tod_zh_field(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    session: &str,
    column: &str,
) -> Result<String, String> {
    // `column` is a fixed internal literal ("focus_zh"), never user input.
    let sql = format!(
        "SELECT COALESCE({column}, '') FROM timesofday \
         WHERE plan_id = ?1 AND destination = ?2 AND day_number = ?3 AND session_type = ?4"
    );
    let mut rows = conn
        .query(
            &sql,
            libsql::params![
                plan_id.to_string(),
                destination.to_string(),
                day,
                session.to_string()
            ],
        )
        .await
        .map_err(|e| e.to_string())?;
    if let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
        return Ok(row.get::<String>(0).unwrap_or_default());
    }
    Ok(String::new())
}

// ─────────────────────────────────────────────────────────────────
// set-tod-time-range / set-session-time-range
// ─────────────────────────────────────────────────────────────────

pub async fn run_time_range(
    args: &[String],
    plan_id: String,
) -> Result<(), String> {
    let parsed = parse_time_range(args)?;
    let start = match &parsed.start {
        Some(s) => s.clone(),
        None => {
            eprintln!("Error: set-session-time-range requires --start HH:MM and --end HH:MM");
            std::process::exit(1);
        }
    };
    let end = match &parsed.end {
        Some(e) => e.clone(),
        None => {
            eprintln!("Error: set-session-time-range requires --start HH:MM and --end HH:MM");
            std::process::exit(1);
        }
    };
    let conn = crate::db::connect_write().await?;
    let destination = match read_destination(&conn, &plan_id, &parsed.dest).await {
        Ok(d) => d,
        Err(e) => return Err(e),
    };
    require_session(&conn, &plan_id, &destination, parsed.day, &parsed.session).await?;

    println!(
        "\n🕒 Setting session time range:\n   Destination: {destination}\n   Day {} {}: {} → {}",
        parsed.day, parsed.session, start, end
    );

    let kv: Vec<(&str, String)> = vec![
        ("day_number", parsed.day.to_string()),
        ("session", parsed.session.clone()),
        ("start", start.clone()),
        ("end", end.clone()),
    ];
    // Order above is correct (matches TS Object.entries insertion order).

    // Touch the day and apply the time_range update.
    touch_day(&conn, &plan_id, &destination, parsed.day).await?;
    apply_time_range(
        &conn,
        &plan_id,
        &destination,
        parsed.day,
        &parsed.session,
        &start,
        &end,
    )
    .await?;

    execute_tod(
        &conn,
        &plan_id,
        &destination,
        parsed.day,
        &parsed.session,
        None,
        None,
        None,
        None,
        &kv,
        "session_time_range_updated",
        "set-session-time-range",
        &format!("D{} {}", parsed.day, parsed.session),
    )
    .await?;

    println!("✅ Session time range updated");
    Ok(())
}

// ─────────────────────────────────────────────────────────────────
// set-tod-zh / set-session-zh
// ─────────────────────────────────────────────────────────────────

pub async fn run_zh(
    args: &[String],
    plan_id: String,
) -> Result<(), String> {
    let parsed = parse_zh(args)?;

    // Write-time map-link guard for the transit pill. transit_notes_zh renders as
    // a clickable Google-Maps "A → B → C" chain on the dashboard; every stop in
    // the chain must form a valid Maps query. This is the SAME guard set-route-
    // segment uses — closing the hole that let broken transit pills (the bug that
    // recurred 3×) reach the dashboard through this writer.
    if let Some(z) = &parsed.transit_zh {
        for stop in z.split('\u{2192}') {
            // Skip blank segments (a trailing/leading → produces an empty piece).
            if stop.trim().is_empty() {
                continue;
            }
            if let Err(reason) = crate::checks::check_stop_linkable(stop) {
                return Err(format!(
                    "transit_notes_zh stop \"{}\" {reason}.\nHint: write the pill as a clean place chain, e.g. \"安里駅 → 赤嶺駅 → iias 沖縄豊崎\" — no （…）notes, +步行, or clock times inside a stop.",
                    stop.trim()
                ));
            }
        }
    }

    let conn = crate::db::connect_write().await?;
    let destination = match read_destination(&conn, &plan_id, &parsed.dest).await {
        Ok(d) => d,
        Err(e) => return Err(e),
    };
    require_session(&conn, &plan_id, &destination, parsed.day, &parsed.session).await?;

    println!(
        "\n🌏 Setting session ZH content:\n   Destination: {}, Day {}, {}",
        destination, parsed.day, parsed.session
    );
    if let Some(z) = &parsed.focus_zh {
        println!("   focus_zh: \"{z}\"");
    }
    if let Some(z) = &parsed.transit_zh {
        println!("   transit_notes_zh: \"{z}\"");
    }
    if let Some(a) = &parsed.activities_zh {
        println!("   activities_zh: {} items", a.len());
    }

    // Apply scalar updates to timesofday.
    if let Some(z) = &parsed.focus_zh {
        apply_tod_field(
            &conn,
            &plan_id,
            &destination,
            parsed.day,
            &parsed.session,
            "focus_zh",
            Some(z.clone()),
        )
        .await?;
    }
    if let Some(z) = &parsed.transit_zh {
        apply_tod_field(
            &conn,
            &plan_id,
            &destination,
            parsed.day,
            &parsed.session,
            "transit_notes_zh",
            Some(z.clone()),
        )
        .await?;
    }
    // activities_zh: DELETE-then-reinsert session_activities_zh. `--clear-activities`
    // does the DELETE with no reinsert (empties the list without re-passing it).
    if parsed.clear_activities || parsed.activities_zh.is_some() {
        conn.execute(
            "DELETE FROM session_activities_zh \
             WHERE plan_id = ?1 AND destination = ?2 AND day_number = ?3 AND session_type = ?4",
            libsql::params![
                plan_id.to_string(),
                destination.to_string(),
                parsed.day,
                parsed.session.clone()
            ],
        )
        .await
        .map_err(|e| format!("session_activities_zh DELETE failed: {e}"))?;
        for (i, a) in parsed.activities_zh.as_deref().unwrap_or(&[]).iter().enumerate() {
            conn.execute(
                "INSERT INTO session_activities_zh \
                    (plan_id, destination, day_number, session_type, sort_order, activity) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                libsql::params![
                    plan_id.to_string(),
                    destination.to_string(),
                    parsed.day,
                    parsed.session.clone(),
                    i as i64,
                    a.clone()
                ],
            )
            .await
            .map_err(|e| format!("session_activities_zh INSERT failed: {e}"))?;
        }
    }
    // No ZH meals: session_meals (the EN/display `meal` column) is the single
    // meal source rendered for both languages — the ZH-meal path was dropped in
    // the de-JSON program (no session_meals_zh table). set-tod-zh never touches
    // session_meals; the EN meal row is preserved.
    touch_day(&conn, &plan_id, &destination, parsed.day).await?;

    // Event data: {day_number, session, focus_zh, transit_notes_zh,
    // activities_zh, meals_zh} — all 6 keys present for audit-schema parity.
    // The TS event `data` object carried the raw command fields, JS `undefined`
    // when omitted; eventDataToKv does String(undefined) → "undefined" (omitted
    // zh fields all serialize as the literal "undefined", NOT "null"). meals_zh
    // is now always "undefined" (the ZH-meal flag was removed) but the key is
    // retained so the event shape is unchanged.
    let kv: Vec<(&str, String)> = vec![
        ("day_number", parsed.day.to_string()),
        ("session", parsed.session.clone()),
        (
            "focus_zh",
            parsed.focus_zh.clone().unwrap_or_else(|| "undefined".to_string()),
        ),
        (
            "transit_notes_zh",
            parsed.transit_zh.clone().unwrap_or_else(|| "undefined".to_string()),
        ),
        (
            "activities_zh",
            parsed
                .activities_zh
                .as_ref()
                .map(|a| a.join(", "))
                .unwrap_or_else(|| "undefined".to_string()),
        ),
        // meals_zh flag removed; key retained as "undefined" for event parity.
        ("meals_zh", "undefined".to_string()),
    ];

    execute_tod(
        &conn,
        &plan_id,
        &destination,
        parsed.day,
        &parsed.session,
        None,
        None,
        None,
        None,
        &kv,
        "itinerary_session_zh_content_set",
        "set-session-zh",
        &format!("D{}/{}", parsed.day, parsed.session),
    )
    .await?;

    println!("✅ Session ZH content updated");
    Ok(())
}

// ── set-meals ──────────────────────────────────────────────────────
//
// Replace the session_meals (base, display) rows for a (day, session) with the
// provided --meal values, in order. This is the missing write path for base
// meals (set-tod-zh only ever touched the ZH activity list, never meals).
//
//   travel set-meals <day> <session> --meal "Lunch: …" [--meal "Dinner: …"] [--dest]
//
// A meal string may carry a map pin via the "<label>｜map:<query>" convention,
// which the dashboard renders as a clickable place link.
pub async fn run_meals(args: &[String], plan_id: String) -> Result<(), String> {
    let parsed = parse_meals(args)?;
    let conn = crate::db::connect_write().await?;
    let destination = read_destination(&conn, &plan_id, &parsed.dest).await?;
    require_session(&conn, &plan_id, &destination, parsed.day, &parsed.session).await?;

    println!(
        "\n🍜 Setting meals:\n   Destination: {}, Day {}, {} — {} meal(s)",
        destination,
        parsed.day,
        parsed.session,
        parsed.meals.len()
    );

    conn.execute(
        "DELETE FROM session_meals \
         WHERE plan_id = ?1 AND destination = ?2 AND day_number = ?3 AND session_type = ?4",
        libsql::params![
            plan_id.to_string(),
            destination.to_string(),
            parsed.day,
            parsed.session.clone()
        ],
    )
    .await
    .map_err(|e| format!("session_meals DELETE failed: {e}"))?;
    for (i, meal) in parsed.meals.iter().enumerate() {
        conn.execute(
            "INSERT INTO session_meals \
                (plan_id, destination, day_number, session_type, sort_order, meal) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            libsql::params![
                plan_id.to_string(),
                destination.to_string(),
                parsed.day,
                parsed.session.clone(),
                i as i64,
                meal.clone()
            ],
        )
        .await
        .map_err(|e| format!("session_meals INSERT failed: {e}"))?;
        println!("   • {meal}");
    }
    touch_day(&conn, &plan_id, &destination, parsed.day).await?;

    let kv: Vec<(&str, String)> = vec![
        ("day_number", parsed.day.to_string()),
        ("session", parsed.session.clone()),
        ("count", parsed.meals.len().to_string()),
        ("meals", parsed.meals.join(" | ")),
    ];
    execute_tod(
        &conn,
        &plan_id,
        &destination,
        parsed.day,
        &parsed.session,
        None,
        None,
        None,
        None,
        &kv,
        "itinerary_session_meals_set",
        "set-meals",
        &format!("D{}/{}", parsed.day, parsed.session),
    )
    .await?;

    println!("✅ Meals updated");
    Ok(())
}

// ─────────────────────────────────────────────────────────────────
// Arg parsing
// ─────────────────────────────────────────────────────────────────

#[derive(Default, Debug)]
struct ParsedMeals {
    day: i64,
    session: String,
    meals: Vec<String>,
    dest: Option<String>,
}

fn parse_meals(args: &[String]) -> Result<ParsedMeals, String> {
    let mut p = ParsedMeals::default();
    let mut positional: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        match a.as_str() {
            "--dest" => {
                p.dest = Some(
                    args.get(i + 1)
                        .ok_or_else(|| "missing value for --dest".to_string())?
                        .clone(),
                );
                i += 2;
            }
            "--meal" => {
                p.meals.push(
                    args.get(i + 1)
                        .ok_or_else(|| "missing value for --meal".to_string())?
                        .clone(),
                );
                i += 2;
            }
            "--plan-id" => {
                i += 2; // consumed by the top-level resolver
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
    if positional.len() < 2 {
        return Err(
            "Usage: set-meals <day> <session> --meal \"<text>\" [--meal \"<text>\"...] [--dest <slug>]"
                .to_string(),
        );
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
    if p.meals.is_empty() {
        return Err("at least one --meal \"<text>\" is required".to_string());
    }
    Ok(p)
}

fn parse_focus(args: &[String]) -> Result<ParsedFocus, String> {
    let mut p = ParsedFocus::default();
    let mut positional: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        match a.as_str() {
            "--dest" => {
                p.dest = Some(
                    args.get(i + 1)
                        .ok_or_else(|| "missing value for --dest".to_string())?
                        .clone(),
                );
                i += 2;
            }
            // Paired ZH: set focus AND focus_zh in one command. The dashboard
            // renders focus_zh by default, so without this the ZH focus would
            // stay stale (the very reason warn_zh_stale exists).
            "--zh" => {
                p.focus_zh = Some(
                    args.get(i + 1)
                        .ok_or_else(|| "missing value for --zh".to_string())?
                        .clone(),
                );
                i += 2;
            }
            "--plan-id" => {
                // consumed by the top-level plan resolver; skip flag + value
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
    if positional.is_empty() {
        return Err("Usage: set-tod-focus <day> <session> \"<focus_text>\"".to_string());
    }
    p.day = positional[0]
        .parse::<i64>()
        .map_err(|_| "<day> must be a positive integer".to_string())?;
    if p.day < 1 {
        return Err("<day> must be a positive integer".to_string());
    }
    if positional.len() < 2 {
        return Err("Usage: set-tod-focus <day> <session> \"<focus_text>\"".to_string());
    }
    p.session = positional[1].clone();
    if !["morning", "noon", "afternoon", "evening"].contains(&p.session.as_str()) {
        return Err("<session> must be one of: morning|noon|afternoon|evening".to_string());
    }
    if positional.len() >= 3 {
        p.focus = Some(positional[2..].join(" "));
    }
    Ok(p)
}

fn parse_time_range(args: &[String]) -> Result<ParsedTimeRange, String> {
    let mut p = ParsedTimeRange::default();
    let mut positional: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        match a.as_str() {
            "--dest" => {
                p.dest = Some(
                    args.get(i + 1)
                        .ok_or_else(|| "missing value for --dest".to_string())?
                        .clone(),
                );
                i += 2;
            }
            "--start" => {
                p.start = Some(
                    args.get(i + 1)
                        .ok_or_else(|| "missing value for --start".to_string())?
                        .clone(),
                );
                i += 2;
            }
            "--end" => {
                p.end = Some(
                    args.get(i + 1)
                        .ok_or_else(|| "missing value for --end".to_string())?
                        .clone(),
                );
                i += 2;
            }
            "--plan-id" => {
                // consumed by the top-level plan resolver; skip flag + value
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
    if positional.len() < 2 {
        return Err("Usage: set-session-time-range <day> <session> --start HH:MM --end HH:MM".to_string());
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
    // Fail-loud HH:MM validation BEFORE any DB write (this Err bubbles to main →
    // stderr + non-zero exit, writing NOTHING). run_time_range requires BOTH
    // --start and --end; when both are present here, also enforce start ≤ end.
    if let Some(s) = &p.start {
        crate::checks::validate_time_flag("--start", s)?;
    }
    if let Some(e) = &p.end {
        crate::checks::validate_time_flag("--end", e)?;
    }
    if let (Some(s), Some(e)) = (&p.start, &p.end) {
        crate::checks::validate_start_le_end("--start", s, "--end", e)?;
    }
    Ok(p)
}

fn parse_zh(args: &[String]) -> Result<ParsedZh, String> {
    let mut p = ParsedZh::default();
    let mut positional: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        match a.as_str() {
            "--dest" => {
                p.dest = Some(
                    args.get(i + 1)
                        .ok_or_else(|| "missing value for --dest".to_string())?
                        .clone(),
                );
                i += 2;
            }
            "--zh" => {
                p.focus_zh = Some(
                    args.get(i + 1)
                        .ok_or_else(|| "missing value for --zh".to_string())?
                        .clone(),
                );
                i += 2;
            }
            "--transit-zh" => {
                p.transit_zh = Some(
                    args.get(i + 1)
                        .ok_or_else(|| "missing value for --transit-zh".to_string())?
                        .clone(),
                );
                i += 2;
            }
            // Plain-text, agent-first: repeat the flag once per item instead of
            // passing a JSON array. `--activity-zh "A" --activity-zh "B"` →
            // ["A", "B"]. First occurrence starts the list (so it overwrites
            // any prior value, matching the old set-then-replace semantics).
            "--activity-zh" => {
                let v = args
                    .get(i + 1)
                    .ok_or_else(|| "missing value for --activity-zh".to_string())?
                    .clone();
                p.activities_zh.get_or_insert_with(Vec::new).push(v);
                i += 2;
            }
            "--clear-activities" => {
                p.clear_activities = true;
                i += 1;
            }
            "--plan-id" => {
                // consumed by the top-level plan resolver; skip flag + value
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
    if positional.len() < 2 {
        return Err("Usage: set-session-zh <day> <session> [--zh \"...\"] [--transit-zh \"...\"] [--activity-zh \"...\" ...] [--clear-activities] [--dest slug]".to_string());
    }
    if p.clear_activities && p.activities_zh.is_some() {
        return Err("--clear-activities cannot be combined with --activity-zh".to_string());
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
    Ok(p)
}

// ─────────────────────────────────────────────────────────────────
// Shared DB helpers
// ─────────────────────────────────────────────────────────────────

async fn read_destination(
    conn: &Connection,
    plan_id: &str,
    dest_opt: &Option<String>,
) -> Result<String, String> {
    crate::cascade::common::resolve_active_destination(conn, plan_id, dest_opt.as_deref()).await
}

#[allow(clippy::too_many_arguments)]
async fn execute_tod(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    session: &str,
    focus_field: Option<&str>,
    focus_zh_field: Option<&str>,
    transit_zh_field: Option<&str>,
    times: Option<(&str, &str)>,
    kv: &[(&str, String)],
    event_name: &str,
    command_type: &str,
    command_summary: &str,
) -> Result<(), String> {
    let _ = (focus_field, focus_zh_field, transit_zh_field, times);
    // The function fields are unused in the executor; the field
    // application happens in the calling run_*() function before
    // this is called. We accept them for symmetry but ignore them
    // here.
    let now_iso = now_rfc3339();
    let now_db = now_db_datetime();

    let version_before = read_version(conn, plan_id).await?;
    let version_after = version_before + 1;

    let dest_process_so = next_dest_process_sort_order(
        conn,
        plan_id,
        destination,
        "process_5_daily_itinerary",
    )
    .await?;
    let timeline_base = next_timeline_sort_order(conn, plan_id).await?;

    insert_event(
        conn,
        plan_id,
        "dest_process",
        destination,
        "process_5_daily_itinerary",
        dest_process_so,
        event_name,
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
        kv,
    )
    .await?;

    let timeline_so = timeline_base;
    insert_event(
        conn,
        plan_id,
        "timeline",
        "",
        "process_5_daily_itinerary",
        timeline_so,
        event_name,
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
        kv,
    )
    .await?;

    crate::cascade::common::record_operation(
        conn,
        plan_id,
        command_type,
        command_summary,
        version_before,
        version_after,
        &now_db,
    )
    .await?;

    let _ = (day, session);
    Ok(())
}

async fn apply_tod_field(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    session: &str,
    field: &str,
    value: Option<String>,
) -> Result<(), String> {
    // Mirror the TS sync INSERT OR REPLACE pattern. The Rust
    // equivalent for an UPDATE of a single column.
    let column = match field {
        "focus" => "focus",
        "focus_zh" => "focus_zh",
        "transit_notes_zh" => "transit_notes_zh",
        _ => return Err(format!("unsupported timesofday field: {field}")),
    };
    let sql = format!(
        "UPDATE timesofday SET {column} = ?1, updated_at = ?2 \
         WHERE plan_id = ?3 AND destination = ?4 AND day_number = ?5 AND session_type = ?6"
    );
    conn.execute(
        &sql,
        libsql::params![
            value,
            now_db_datetime(),
            plan_id.to_string(),
            destination.to_string(),
            day,
            session.to_string()
        ],
    )
    .await
    .map_err(|e| format!("timesofday UPDATE {field} failed: {e}"))?;
    touch_day(conn, plan_id, destination, day).await?;
    Ok(())
}

async fn apply_time_range(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    session: &str,
    start: &str,
    end: &str,
) -> Result<(), String> {
    conn.execute(
        "UPDATE timesofday SET time_range_start = ?1, time_range_end = ?2, updated_at = ?3 \
         WHERE plan_id = ?4 AND destination = ?5 AND day_number = ?6 AND session_type = ?7",
        libsql::params![
            start.to_string(),
            end.to_string(),
            now_db_datetime(),
            plan_id.to_string(),
            destination.to_string(),
            day,
            session.to_string()
        ],
    )
    .await
    .map_err(|e| format!("timesofday time_range UPDATE failed: {e}"))?;
    Ok(())
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

/// Fail loud unless the target `timesofday` session row pre-exists. These
/// commands UPDATE an existing session (created by the itinerary scaffold);
/// without this guard a missing session would silently no-op the field UPDATE
/// while still writing a `completed` audit + events (and, for set-tod-zh,
/// orphan `session_activities_zh` child rows). Called early in each run_*()
/// before ANY write so a ✅/completed audit always implies a row changed.
async fn require_session(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    session: &str,
) -> Result<(), String> {
    let mut rows = conn
        .query(
            "SELECT 1 FROM timesofday \
             WHERE plan_id = ?1 AND destination = ?2 AND day_number = ?3 AND session_type = ?4",
            libsql::params![
                plan_id.to_string(),
                destination.to_string(),
                day,
                session.to_string()
            ],
        )
        .await
        .map_err(|e| format!("timesofday existence query failed: {e}"))?;
    let exists = rows
        .next()
        .await
        .map_err(|e| format!("timesofday existence row read failed: {e}"))?
        .is_some();
    if !exists {
        return Err(format!(
            "no timesofday row for plan={plan_id} destination={destination} day={day} \
             session={session}; scaffold the itinerary first (travel scaffold-itinerary)"
        ));
    }
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
        let v: i64 = row
            .get(0)
            .map_err(|e| format!("version col read failed: {e}"))?;
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
        let m: i64 = row
            .get(0)
            .map_err(|e| format!("plan_events MAX col read failed: {e}"))?;
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

#[allow(clippy::too_many_arguments)]
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
    format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{min:02}:{sec:02}.{ms:03}Z"
    )
}

fn now_db_datetime() -> String {
    let (year, month, day, hour, min, sec, _) = now_civil();
    format!(
        "{year:04}-{month:02}-{day:02} {hour:02}:{min:02}:{sec:02}"
    )
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

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    // ── HH:MM fail-loud validation (set-tod-time-range) ───────────────
    #[test]
    fn parse_time_range_accepts_valid_clocks() {
        let p = parse_time_range(&s(&["1", "morning", "--start", "08:00", "--end", "12:00"]))
            .unwrap();
        assert_eq!(p.start.as_deref(), Some("08:00"));
        assert_eq!(p.end.as_deref(), Some("12:00"));
    }

    #[test]
    fn parse_time_range_rejects_bad_start() {
        let err = parse_time_range(&s(&["1", "morning", "--start", "8am", "--end", "12:00"]))
            .unwrap_err();
        assert!(err.contains("--start") && err.contains("\"8am\""), "got: {err}");
    }

    #[test]
    fn parse_time_range_rejects_bad_end() {
        let err = parse_time_range(&s(&["1", "morning", "--start", "08:00", "--end", "24:00"]))
            .unwrap_err();
        assert!(err.contains("--end") && err.contains("\"24:00\""), "got: {err}");
    }

    #[test]
    fn parse_time_range_rejects_start_after_end() {
        let err = parse_time_range(&s(&["1", "morning", "--start", "13:00", "--end", "09:00"]))
            .unwrap_err();
        assert!(err.contains("start must be"), "got: {err}");
    }

    #[test]
    fn parse_focus_basic() {
        let p = parse_focus(&[
            "1".to_string(),
            "morning".to_string(),
            "Tokyo Tower visit".to_string(),
        ])
        .unwrap();
        assert_eq!(p.day, 1);
        assert_eq!(p.session, "morning");
        assert_eq!(p.focus.as_deref(), Some("Tokyo Tower visit"));
    }

    #[test]
    fn parse_focus_no_text_clears() {
        let p = parse_focus(&["2".to_string(), "noon".to_string()]).unwrap();
        assert!(p.focus.is_none());
    }

    #[test]
    fn parse_focus_bad_session() {
        assert!(parse_focus(&["1".to_string(), "nope".to_string()]).is_err());
    }

    #[test]
    fn parse_zh_repeatable_activity_flags() {
        // Plain-text, agent-first: --activity-zh repeats instead of a JSON array.
        let p = parse_zh(&[
            "1".to_string(),
            "morning".to_string(),
            "--activity-zh".to_string(),
            "晴空塔".to_string(),
            "--activity-zh".to_string(),
            "淺草寺".to_string(),
        ])
        .unwrap();
        assert_eq!(
            p.activities_zh.as_deref(),
            Some(["晴空塔".to_string(), "淺草寺".to_string()].as_slice())
        );
    }

    #[test]
    fn parse_zh_rejects_legacy_json_flags() {
        // Both JSON-array flags are gone; they must be reported as unknown,
        // not parsed. (meals_zh had no live ZH-meal storage — session_meals
        // is the single meal source rendered for both languages — so the
        // meal ZH flag was dropped entirely rather than left half-working.)
        for flag in ["--activities-zh-json", "--meals-zh-json", "--meal-zh"] {
            let err = parse_zh(&[
                "1".to_string(),
                "morning".to_string(),
                flag.to_string(),
                r#"["a","b"]"#.to_string(),
            ])
            .unwrap_err();
            assert!(err.contains("unknown argument"), "flag {flag} got: {err}");
        }
    }
}
