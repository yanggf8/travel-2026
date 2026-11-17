// `travel record-activity` / `record-meal` / `record-leg` / `record-session` /
// `clear-record` — mark the ACTUAL outcome of a past trip item
// (docs/superpowers/specs/2026-11-16-trip-record-design.md §5). One row per
// command, audit triad in one BEGIN IMMEDIATE transaction; same-value rerun
// writes nothing (no version bump). Never touches titles: past items keep
// their identity, the record lives in the record_* columns.
//
// Guards (spec §5): variant days are refused (alternative-plan content is
// never recorded), and days dated after the destination-local today are
// refused ("today" computed from destination_config.timezone — NOT
// plan_resolver::today_iso(), which is UTC).

use libsql::Connection;
use travel_db::repo::itinerary;
use travel_db::repo::records::{self, RecordStatus};

use crate::cascade::common::{
    begin_immediate, finish_tx, insert_event, insert_kv_rows, next_dest_process_sort_order,
    next_timeline_sort_order, now_db_datetime, now_rfc3339, read_version, record_operation,
    resolve_active_destination,
};

const PROCESS: &str = "process_5_daily_itinerary";

pub const ACTIVITY_USAGE: &str = "travel record-activity <day> <session> <id|title> done|skipped [--note \"<t>\"] [--dest slug]\n  (mark what actually happened to one activity; --note optional, omitted = keep current)";
pub const MEAL_USAGE: &str = "travel record-meal <day> <session> <text-match> done|skipped [--note \"<t>\"] [--dest slug]\n  (mark one meal by unique text match)";
pub const LEG_USAGE: &str = "travel record-leg <day> <sort_order> done|skipped [--note \"<t>\"] [--dest slug]\n  (mark one route segment by its day + sort_order slot)";
pub const SESSION_USAGE: &str = "travel record-session <day> <session> --zh \"<t>\" | --clear [--dest slug]\n  (write the ZH actual summary of one past session; replaces the old in-focus hack)";
pub const CLEAR_USAGE: &str = "travel clear-record activity|meal|leg <day> ... <key> [--dest slug]\n  activity: clear-record activity <day> <session> <id|title>\n  meal:     clear-record meal <day> <session> <text-match>\n  leg:      clear-record leg <day> <sort_order>\n  (reset record_status AND record_note to NULL; audited when something was set)";

/// The destination-local today, `YYYY-MM-DD` (spec §5, K3#9): `TRAVEL_TODAY`
/// wins (test seam, mirrors `plan_resolver::today_iso`), else the system
/// `date(1)` runs with `TZ=<destination_config.timezone>` (the zoneinfo
/// validation mirrors `plan::format_in_tz`). A NULL/empty/missing timezone is
/// a loud error — never a silent fallback to UTC.
pub(crate) async fn destination_today(conn: &Connection, dest: &str) -> Result<String, String> {
    if let Ok(v) = std::env::var("TRAVEL_TODAY")
        && !v.trim().is_empty()
    {
        return Ok(v);
    }
    let tz: Option<String> = {
        let mut rows = conn
            .query(
                "SELECT timezone FROM destination_config WHERE slug = ?1",
                libsql::params![dest.to_string()],
            )
            .await
            .map_err(|e| format!("destination_config query failed: {e}"))?;
        match rows.next().await.map_err(|e| e.to_string())? {
            Some(row) => row.get::<Option<String>>(0).map_err(|e| e.to_string())?,
            None => None,
        }
    };
    let Some(tz) = tz.filter(|t| !t.trim().is_empty()) else {
        return Err(format!(
            "destination_config.timezone is missing, NULL or empty for '{dest}' — cannot \
             determine the destination-local today (needed to refuse recording future days). \
             Register the destination (destination_config.timezone) or set TRAVEL_TODAY."
        ));
    };
    if tz.contains("..")
        || tz.starts_with('/')
        || !std::path::Path::new("/usr/share/zoneinfo").join(&tz).is_file()
    {
        return Err(format!(
            "unknown timezone \"{tz}\" (destination_config.timezone for '{dest}')"
        ));
    }
    let out = std::process::Command::new("date")
        .env("TZ", &tz)
        .arg("+%F")
        .output()
        .map_err(|e| format!("failed to run date(1): {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "cannot compute the destination-local today in TZ={tz}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Shared day guards: the day must exist, be a MAIN day (variants are never
/// recorded) and be dated on/before the destination-local today.
async fn check_day(
    conn: &Connection,
    plan_id: &str,
    dest: &str,
    day: i64,
    today: &str,
) -> Result<(), String> {
    if let Some(main) = travel_db::repo::days::variant_parent(conn, plan_id, dest, day).await? {
        return Err(format!(
            "day {day} is a VARIANT of day {main} — variant rows cannot be recorded \
             (record on the main day {main})"
        ));
    }
    let date: String = {
        let mut rows = conn
            .query(
                "SELECT date FROM days WHERE plan_id = ?1 AND destination = ?2 AND day_number = ?3",
                libsql::params![plan_id.to_string(), dest.to_string(), day],
            )
            .await
            .map_err(|e| format!("days query failed: {e}"))?;
        let row = rows
            .next()
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("day {day} does not exist for plan {plan_id} ({dest})"))?;
        row.get(0).map_err(|e| format!("days date read failed: {e}"))?
    };
    if date.as_str() > today {
        return Err(format!(
            "cannot record a future day: day {day} is dated {date}, the destination-local \
             today is {today} (records are for what already happened)"
        ));
    }
    Ok(())
}

/// Emit one `record_marked` / `record_cleared` / edit event pair (dest_process
/// + timeline, like every itinerary write) and write the audit-triad back
/// half. Shared with the T5 edit commands (set_tod). Caller is inside the
/// command transaction.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn emit_record_event(
    conn: &Connection,
    plan_id: &str,
    dest: &str,
    event: &str,
    kv: &[(&str, String)],
    version_before: i64,
    command_type: &str,
    command_summary: &str,
) -> Result<(), String> {
    let now_iso = now_rfc3339();
    let dest_so = next_dest_process_sort_order(conn, plan_id, dest, PROCESS).await?;
    insert_event(
        conn,
        plan_id,
        "dest_process",
        dest,
        PROCESS,
        dest_so,
        event,
        &now_iso,
        None,
        None,
    )
    .await?;
    insert_kv_rows(conn, plan_id, "dest_process", dest, PROCESS, dest_so, kv).await?;
    let tl_so = next_timeline_sort_order(conn, plan_id).await?;
    insert_event(
        conn,
        plan_id,
        "timeline",
        "",
        PROCESS,
        tl_so,
        event,
        &now_iso,
        None,
        None,
    )
    .await?;
    insert_kv_rows(conn, plan_id, "timeline", "", PROCESS, tl_so, kv).await?;
    record_operation(
        conn,
        plan_id,
        command_type,
        command_summary,
        version_before,
        version_before + 1,
        &now_db_datetime(),
    )
    .await?;
    Ok(())
}

/// The common positional/flag scan shared by the record commands.
struct Common {
    day: i64,
    note: Option<String>,
    dest: Option<String>,
    zh: Option<String>,
    clear: bool,
    rest: Vec<String>,
}

fn parse_common(
    args: &[String],
    n_positional: usize,
    usage: &str,
    flags: &[&str],
) -> Result<Common, String> {
    let mut c = Common {
        day: 0,
        note: None,
        dest: None,
        zh: None,
        clear: false,
        rest: Vec::new(),
    };
    let mut positional: Vec<String> = Vec::new();
    let value_flags: &[&str] = flags;
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        let take_value = |i: &mut usize, slot: &mut Option<String>| -> Result<(), String> {
            *slot = Some(
                args.get(*i + 1)
                    .ok_or_else(|| "missing value after flag".to_string())?
                    .clone(),
            );
            *i += 2;
            Ok(())
        };
        match a {
            "--dest" => take_value(&mut i, &mut c.dest)?,
            "--note" => take_value(&mut i, &mut c.note)?,
            "--zh" => take_value(&mut i, &mut c.zh)?,
            "--clear" => {
                c.clear = true;
                i += 1;
            }
            f if crate::plan_resolver::is_resolver_flag(f) => i += 2,
            other if value_flags.contains(&other) => {
                return Err(format!("flag {other} is not valid here. Usage:\n{usage}"));
            }
            other if other.starts_with("--") => {
                return Err(format!("unknown argument: {other}. Usage:\n{usage}"));
            }
            _ => {
                positional.push(a.to_string());
                i += 1;
            }
        }
    }
    if positional.len() != n_positional {
        return Err(format!("Usage:\n{usage}"));
    }
    c.day = positional[0]
        .parse::<i64>()
        .map_err(|_| format!("<day> must be a positive integer. Usage:\n{usage}"))?;
    if c.day < 1 {
        return Err(format!("<day> must be a positive integer. Usage:\n{usage}"));
    }
    c.rest = positional;
    Ok(c)
}

fn parse_status(s: &str, usage: &str) -> Result<RecordStatus, String> {
    RecordStatus::parse(s).map_err(|e| format!("{e}. Usage:\n{usage}"))
}

/// record-activity <day> <session> <id|title> done|skipped [--note]
pub async fn run_activity(args: &[String], plan_id: String) -> Result<(), String> {
    let c = parse_common(args, 4, ACTIVITY_USAGE, &[])?;
    let session = c.rest[1].clone();
    let key = c.rest[2].clone();
    let status = parse_status(&c.rest[3], ACTIVITY_USAGE)?;

    let conn = crate::db::connect_write().await?;
    let destination = resolve_active_destination(&conn, &plan_id, c.dest.as_deref()).await?;
    let today = destination_today(&conn, &destination).await?;
    check_day(&conn, &plan_id, &destination, c.day, &today).await?;
    let id =
        records::resolve_activity_unique(&conn, &plan_id, &destination, c.day, &session, &key)
            .await?;

    println!("\n📝 Recording activity: D{}/{} {} → {}", c.day, session, key, status.as_str());
    let outcome = async {
        begin_immediate(&conn, "record-activity").await?;
        let version_before = read_version(&conn, &plan_id).await?;
        let changed = records::set_activity_record(
            &conn,
            &plan_id,
            &destination,
            &id,
            Some(status),
            c.note.as_deref().map(Some),
        )
        .await?;
        if !changed {
            return Ok(false);
        }
        // Map-freshness inputs: the activity row's updated_at moved inside
        // set_activity_record; touch the owning day too (spec §5, Fable#11).
        itinerary::touch_day(&conn, &plan_id, &destination, c.day, &now_db_datetime()).await?;
        emit_record_event(
            &conn,
            &plan_id,
            &destination,
            "record_marked",
            &[
                ("target", "activity".to_string()),
                ("activity_id", id.clone()),
                ("day_number", c.day.to_string()),
                ("session", session.clone()),
                ("status", status.as_str().to_string()),
                (
                    "note",
                    c.note.clone().unwrap_or_else(|| "undefined".to_string()),
                ),
            ],
            version_before,
            "record-activity",
            &format!("D{}/{}: {} → {}", c.day, session, key, status.as_str()),
        )
        .await?;
        Ok(true)
    }
    .await;
    let changed = finish_tx(&conn, "record-activity", outcome).await?;
    if changed {
        println!("✅ Recorded");
    } else {
        println!("ℹ️ No change (same status/note) — nothing written");
    }
    Ok(())
}

/// record-meal <day> <session> <text-match> done|skipped [--note]
pub async fn run_meal(args: &[String], plan_id: String) -> Result<(), String> {
    let c = parse_common(args, 4, MEAL_USAGE, &[])?;
    let session = c.rest[1].clone();
    let key = c.rest[2].clone();
    let status = parse_status(&c.rest[3], MEAL_USAGE)?;

    let conn = crate::db::connect_write().await?;
    let destination = resolve_active_destination(&conn, &plan_id, c.dest.as_deref()).await?;
    let today = destination_today(&conn, &destination).await?;
    check_day(&conn, &plan_id, &destination, c.day, &today).await?;
    let sort_order =
        records::resolve_meal_unique(&conn, &plan_id, &destination, c.day, &session, &key).await?;

    println!("\n📝 Recording meal: D{}/{} {} → {}", c.day, session, key, status.as_str());
    let outcome = async {
        begin_immediate(&conn, "record-meal").await?;
        let version_before = read_version(&conn, &plan_id).await?;
        let changed = records::set_meal_record(
            &conn,
            &plan_id,
            &destination,
            c.day,
            &session,
            sort_order,
            Some(status),
            c.note.as_deref().map(Some),
        )
        .await?;
        if !changed {
            return Ok(false);
        }
        emit_record_event(
            &conn,
            &plan_id,
            &destination,
            "record_marked",
            &[
                ("target", "meal".to_string()),
                ("sort_order", sort_order.to_string()),
                ("day_number", c.day.to_string()),
                ("session", session.clone()),
                ("status", status.as_str().to_string()),
                (
                    "note",
                    c.note.clone().unwrap_or_else(|| "undefined".to_string()),
                ),
            ],
            version_before,
            "record-meal",
            &format!("D{}/{} meal {} → {}", c.day, session, key, status.as_str()),
        )
        .await?;
        Ok(true)
    }
    .await;
    let changed = finish_tx(&conn, "record-meal", outcome).await?;
    if changed {
        println!("✅ Recorded");
    } else {
        println!("ℹ️ No change (same status/note) — nothing written");
    }
    Ok(())
}

/// record-leg <day> <sort_order> done|skipped [--note]
pub async fn run_leg(args: &[String], plan_id: String) -> Result<(), String> {
    let c = parse_common(args, 3, LEG_USAGE, &[])?;
    let sort_order: i64 = c.rest[1]
        .parse()
        .map_err(|_| format!("<sort_order> must be an integer. Usage:\n{LEG_USAGE}"))?;
    let status = parse_status(&c.rest[2], LEG_USAGE)?;

    let conn = crate::db::connect_write().await?;
    let destination = resolve_active_destination(&conn, &plan_id, c.dest.as_deref()).await?;
    let today = destination_today(&conn, &destination).await?;
    check_day(&conn, &plan_id, &destination, c.day, &today).await?;

    println!("\n📝 Recording leg: D{} #{} → {}", c.day, sort_order, status.as_str());
    let outcome = async {
        begin_immediate(&conn, "record-leg").await?;
        let version_before = read_version(&conn, &plan_id).await?;
        let changed = records::set_leg_record(
            &conn,
            &plan_id,
            &destination,
            c.day,
            sort_order,
            Some(status),
            c.note.as_deref().map(Some),
        )
        .await?;
        if !changed {
            return Ok(false);
        }
        // day_route_segments has no updated_at column — touching the day is the
        // only freshness signal for a leg edit.
        itinerary::touch_day(&conn, &plan_id, &destination, c.day, &now_db_datetime()).await?;
        emit_record_event(
            &conn,
            &plan_id,
            &destination,
            "record_marked",
            &[
                ("target", "leg".to_string()),
                ("sort_order", sort_order.to_string()),
                ("day_number", c.day.to_string()),
                ("status", status.as_str().to_string()),
                (
                    "note",
                    c.note.clone().unwrap_or_else(|| "undefined".to_string()),
                ),
            ],
            version_before,
            "record-leg",
            &format!("D{} leg #{sort_order} → {}", c.day, status.as_str()),
        )
        .await?;
        Ok(true)
    }
    .await;
    let changed = finish_tx(&conn, "record-leg", outcome).await?;
    if changed {
        println!("✅ Recorded");
    } else {
        println!("ℹ️ No change (same status/note) — nothing written");
    }
    Ok(())
}

/// record-session <day> <session> --zh "<t>" | --clear
pub async fn run_session(args: &[String], plan_id: String) -> Result<(), String> {
    let c = parse_common(args, 2, SESSION_USAGE, &[])?;
    let session = c.rest[1].clone();
    if c.zh.is_none() && !c.clear {
        return Err(format!("pass --zh \"<text>\" or --clear. Usage:\n{SESSION_USAGE}"));
    }
    if c.zh.is_some() && c.clear {
        return Err(format!("--zh and --clear are mutually exclusive. Usage:\n{SESSION_USAGE}"));
    }

    let conn = crate::db::connect_write().await?;
    let destination = resolve_active_destination(&conn, &plan_id, c.dest.as_deref()).await?;
    let today = destination_today(&conn, &destination).await?;
    check_day(&conn, &plan_id, &destination, c.day, &today).await?;

    let new_summary = c.zh.clone();
    let day = c.day;
    println!(
        "\n📝 Recording session summary: D{}/{session} {}",
        day,
        if c.clear { "(clear)" } else { "" }
    );
    let outcome = async {
        begin_immediate(&conn, "record-session").await?;
        let version_before = read_version(&conn, &plan_id).await?;
        let changed = records::set_session_summary(
            &conn,
            &plan_id,
            &destination,
            day,
            &session,
            new_summary.as_deref(),
        )
        .await?;
        if !changed {
            return Ok(false);
        }
        emit_record_event(
            &conn,
            &plan_id,
            &destination,
            // --clear is a CLEAR, not a mark: the event type must say so (same
            // event vocabulary as clear-record activity|meal|leg).
            if c.clear { "record_cleared" } else { "record_marked" },
            &[
                ("target", "session".to_string()),
                ("day_number", day.to_string()),
                ("session", session.clone()),
                (
                    "summary_zh",
                    new_summary.clone().unwrap_or_else(|| "undefined".to_string()),
                ),
                ("cleared", if c.clear { "true" } else { "false" }.to_string()),
            ],
            version_before,
            "record-session",
            &format!("D{}/{} summary {}", day, session, if c.clear { "cleared" } else { "set" }),
        )
        .await?;
        Ok(true)
    }
    .await;
    let changed = finish_tx(&conn, "record-session", outcome).await?;
    if changed {
        println!("✅ Recorded");
    } else {
        println!("ℹ️ No change (same summary) — nothing written");
    }
    Ok(())
}

/// clear-record activity|meal|leg <day> ... <key>
pub async fn run_clear(args: &[String], plan_id: String) -> Result<(), String> {
    let Some(target) = args.first() else {
        return Err(format!("Usage:\n{CLEAR_USAGE}"));
    };
    let rest: Vec<String> = args[1..].to_vec();
    match target.as_str() {
        "activity" => {
            let c = parse_common(&rest, 3, CLEAR_USAGE, &[])?;
            let session = c.rest[1].clone();
            let key = c.rest[2].clone();
            let conn = crate::db::connect_write().await?;
            let destination =
                resolve_active_destination(&conn, &plan_id, c.dest.as_deref()).await?;
            let today = destination_today(&conn, &destination).await?;
            check_day(&conn, &plan_id, &destination, c.day, &today).await?;
            let id = records::resolve_activity_unique(
                &conn,
                &plan_id,
                &destination,
                c.day,
                &session,
                &key,
            )
            .await?;
            finish_clear(
                &conn,
                &plan_id,
                &destination,
                c.day,
                &format!("D{}/{}: {}", c.day, session, key),
                ClearOp::Activity { id: &id },
            )
            .await
        }
        "meal" => {
            let c = parse_common(&rest, 3, CLEAR_USAGE, &[])?;
            let session = c.rest[1].clone();
            let key = c.rest[2].clone();
            let conn = crate::db::connect_write().await?;
            let destination =
                resolve_active_destination(&conn, &plan_id, c.dest.as_deref()).await?;
            let today = destination_today(&conn, &destination).await?;
            check_day(&conn, &plan_id, &destination, c.day, &today).await?;
            let sort_order = records::resolve_meal_unique(
                &conn,
                &plan_id,
                &destination,
                c.day,
                &session,
                &key,
            )
            .await?;
            finish_clear(
                &conn,
                &plan_id,
                &destination,
                c.day,
                &format!("D{}/{} meal {}", c.day, session, key),
                ClearOp::Meal { session: &session, sort_order },
            )
            .await
        }
        "leg" => {
            let c = parse_common(&rest, 2, CLEAR_USAGE, &[])?;
            let sort_order: i64 = c.rest[1]
                .parse()
                .map_err(|_| format!("<sort_order> must be an integer. Usage:\n{CLEAR_USAGE}"))?;
            let conn = crate::db::connect_write().await?;
            let destination = resolve_active_destination(&conn, &plan_id, c.dest.as_deref()).await?;
            let today = destination_today(&conn, &destination).await?;
            check_day(&conn, &plan_id, &destination, c.day, &today).await?;
            finish_clear(
                &conn,
                &plan_id,
                &destination,
                c.day,
                &format!("D{} leg #{sort_order}", c.day),
                ClearOp::Leg { sort_order },
            )
            .await
        }
        other => Err(format!(
            "invalid record target \"{other}\" (legal: activity | meal | leg). Usage:\n{CLEAR_USAGE}"
        )),
    }
}

/// The already-resolved key a `clear-record` run writes to (the repo clear
/// happens INSIDE the command transaction).
enum ClearOp<'a> {
    Activity { id: &'a str },
    Meal { session: &'a str, sort_order: i64 },
    Leg { sort_order: i64 },
}

impl ClearOp<'_> {
    fn target(&self) -> &'static str {
        match self {
            ClearOp::Activity { .. } => "activity",
            ClearOp::Meal { .. } => "meal",
            ClearOp::Leg { .. } => "leg",
        }
    }
}

/// One clear-record run: resolve first (read-only), then one transaction —
/// repo clear, `record_cleared` triad when something was actually set.
async fn finish_clear(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    summary: &str,
    op: ClearOp<'_>,
) -> Result<(), String> {
    println!("\n🧹 Clearing {} record: {}", op.target(), summary);
    let outcome = async {
        begin_immediate(conn, "clear-record").await?;
        let version_before = read_version(conn, plan_id).await?;
        let changed = match &op {
            ClearOp::Activity { id } => {
                records::clear_activity_record(conn, plan_id, destination, id).await?
            }
            ClearOp::Meal { session, sort_order } => {
                records::clear_meal_record(conn, plan_id, destination, day, session, *sort_order)
                    .await?
            }
            ClearOp::Leg { sort_order } => {
                records::clear_leg_record(conn, plan_id, destination, day, *sort_order).await?
            }
        };
        if !changed {
            return Ok(false);
        }
        // Map-freshness inputs: the activity row's updated_at already moved
        // inside clear_activity_record; day_route_segments has NO updated_at
        // column, so touching the day is the only freshness signal for a
        // cleared leg (clearing a skipped leg changes the drawn map — the
        // check-maps-fresh hint must fire). Meals and session summaries never
        // change a drawn map.
        if matches!(op, ClearOp::Activity { .. } | ClearOp::Leg { .. }) {
            itinerary::touch_day(conn, plan_id, destination, day, &now_db_datetime()).await?;
        }
        emit_record_event(
            conn,
            plan_id,
            destination,
            "record_cleared",
            &[
                ("target", op.target().to_string()),
                ("day_number", day.to_string()),
            ],
            version_before,
            "clear-record",
            summary,
        )
        .await?;
        Ok(true)
    }
    .await;
    let changed = finish_tx(conn, "clear-record", outcome).await?;
    if changed {
        println!("✅ Cleared");
    } else {
        println!("ℹ️ Nothing recorded there — nothing written");
    }
    Ok(())
}
