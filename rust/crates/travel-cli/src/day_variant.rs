// `travel add-day-variant` / `set-day-variant-label` / `remove-day-variant` — alternative plans
// for one trip day (spec 2026-11-08-day-variants). A variant is a `days` row whose `variant_of`
// points at its main day; it owns the usual child rows (timesofday, activities, ...).
//
// Every command writes the audit triad: plan_events (+ plan_event_data), plans.version bump and
// an operation_runs row (via cascade::common::record_operation).

use libsql::{params, Connection};
use travel_db::repo::days;

use crate::cascade::common::{
    insert_event, insert_kv_rows, next_dest_process_sort_order, next_timeline_sort_order,
    now_db_datetime, now_rfc3339, read_version, record_operation, resolve_active_destination,
};

pub const ADD_USAGE: &str = "travel add-day-variant <day> --label <en> --label-zh <zh> [--main-label <en> --main-label-zh <zh>] [--dest slug] [--plan-id <id>]\n  (create an alternative plan for trip day <day>; the main day needs labels, pass --main-label/--main-label-zh the first time)";
pub const LABEL_USAGE: &str = "travel set-day-variant-label <day> --label <en> --label-zh <zh> [--dest slug] [--plan-id <id>]\n  (set the labels of a main day or a variant)";
pub const REMOVE_USAGE: &str = "travel remove-day-variant <variant_day> [--dest slug] [--plan-id <id>]\n  (delete a variant and all its content; clears the main day's labels when none remain)";

const PROCESS: &str = "process_5_daily_itinerary";

/// EN label: printable ASCII, 1-32 chars (drawn into PNG titles with the ASCII bitmap font).
pub fn check_label_en(s: &str) -> Result<(), String> {
    let n = s.chars().count();
    if n == 0 || n > 32 || s.trim().is_empty() || !s.chars().all(|c| (' '..='~').contains(&c)) {
        return Err(format!(
            "--label must be printable ASCII, 1-32 chars (got {n} chars: {s:?})"
        ));
    }
    Ok(())
}

/// ZH label: 1-16 chars.
pub fn check_label_zh(s: &str) -> Result<(), String> {
    let n = s.chars().count();
    if n == 0 || n > 16 || s.trim().is_empty() {
        return Err(format!("--label-zh must be 1-16 chars (got {n}: {s:?})"));
    }
    Ok(())
}

#[derive(Default)]
struct Parsed {
    day: i64,
    label: Option<String>,
    label_zh: Option<String>,
    main_label: Option<String>,
    main_label_zh: Option<String>,
    dest: Option<String>,
}

fn parse(args: &[String], usage: &str) -> Result<Parsed, String> {
    let mut p = Parsed::default();
    let mut positional: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        let slot = match a {
            "--label" => Some(&mut p.label),
            "--label-zh" => Some(&mut p.label_zh),
            "--main-label" => Some(&mut p.main_label),
            "--main-label-zh" => Some(&mut p.main_label_zh),
            "--dest" => Some(&mut p.dest),
            _ => None,
        };
        if let Some(slot) = slot {
            *slot = Some(
                args.get(i + 1)
                    .ok_or_else(|| format!("missing value for {a}"))?
                    .clone(),
            );
            i += 2;
        } else if crate::plan_resolver::is_resolver_flag(a) {
            i += 2;
        } else if a.starts_with("--") {
            return Err(format!("unknown argument: {a}"));
        } else {
            positional.push(args[i].clone());
            i += 1;
        }
    }
    if positional.len() != 1 {
        return Err(format!("Error: expected exactly one <day>\nUsage: {usage}"));
    }
    p.day = positional[0]
        .parse::<i64>()
        .ok()
        .filter(|d| *d >= 1)
        .ok_or_else(|| "Error: <day> must be a positive integer".to_string())?;
    Ok(p)
}

async fn labels_of(
    conn: &Connection,
    plan: &str,
    dest: &str,
    day: i64,
) -> Result<(Option<String>, Option<String>, String, String), String> {
    let mut rows = conn
        .query(
            "SELECT variant_label, variant_label_zh, date, day_type FROM days \
             WHERE plan_id = ?1 AND destination = ?2 AND day_number = ?3",
            params![plan.to_string(), dest.to_string(), day],
        )
        .await
        .map_err(|e| format!("days read failed: {e}"))?;
    let row = rows
        .next()
        .await
        .map_err(|e| format!("days row read failed: {e}"))?
        .ok_or_else(|| format!("day {day} does not exist for plan {plan} ({dest})"))?;
    let g = |e: libsql::Error| format!("days decode failed: {e}");
    Ok((
        row.get::<Option<String>>(0).map_err(g)?,
        row.get::<Option<String>>(1).map_err(g)?,
        row.get::<String>(2).map_err(g)?,
        row.get::<String>(3).map_err(g)?,
    ))
}

fn nonempty(o: &Option<String>) -> bool {
    o.as_deref().map(|s| !s.trim().is_empty()).unwrap_or(false)
}

async fn set_labels(
    conn: &Connection,
    plan: &str,
    dest: &str,
    day: i64,
    en: Option<&str>,
    zh: Option<&str>,
    now_db: &str,
) -> Result<(), String> {
    conn.execute(
        "UPDATE days SET variant_label = ?4, variant_label_zh = ?5, updated_at = ?6 \
         WHERE plan_id = ?1 AND destination = ?2 AND day_number = ?3",
        params![
            plan.to_string(),
            dest.to_string(),
            day,
            en.map(str::to_string),
            zh.map(str::to_string),
            now_db.to_string()
        ],
    )
    .await
    .map_err(|e| format!("days label update failed: {e}"))?;
    Ok(())
}

/// Emit the dest_process + timeline events (with KV) and the operation_runs/version back half.
async fn audit(
    conn: &Connection,
    plan: &str,
    dest: &str,
    event: &str,
    command: &str,
    summary: &str,
    kv: &[(&str, String)],
) -> Result<(), String> {
    let now_iso = now_rfc3339();
    let now_db = now_db_datetime();
    let before = read_version(conn, plan).await?;
    let dp = next_dest_process_sort_order(conn, plan, dest, PROCESS).await?;
    let tl = next_timeline_sort_order(conn, plan).await?;
    insert_event(conn, plan, "dest_process", dest, PROCESS, dp, event, &now_iso, None, None).await?;
    insert_kv_rows(conn, plan, "dest_process", dest, PROCESS, dp, kv).await?;
    insert_event(conn, plan, "timeline", "", PROCESS, tl, event, &now_iso, None, None).await?;
    insert_kv_rows(conn, plan, "timeline", "", PROCESS, tl, kv).await?;
    record_operation(conn, plan, command, summary, before, before + 1, &now_db).await?;
    Ok(())
}

pub async fn run_add(args: &[String], plan_id: String) -> Result<(), String> {
    let p = parse(args, ADD_USAGE)?;
    let label = p.label.as_deref().ok_or("--label is required")?;
    let label_zh = p.label_zh.as_deref().ok_or("--label-zh is required")?;
    check_label_en(label)?;
    check_label_zh(label_zh)?;
    match (&p.main_label, &p.main_label_zh) {
        (Some(en), Some(zh)) => {
            check_label_en(en).map_err(|e| e.replace("--label", "--main-label"))?;
            check_label_zh(zh).map_err(|e| e.replace("--label-zh", "--main-label-zh"))?;
        }
        (None, None) => {}
        _ => return Err("--main-label and --main-label-zh must be given together".into()),
    }

    let conn = crate::db::connect_write().await?;
    let dest = resolve_active_destination(&conn, &plan_id, p.dest.as_deref()).await?;

    if days::variant_parent(&conn, &plan_id, &dest, p.day).await?.is_some() {
        return Err(format!(
            "day {} is itself a variant; add the variant to its main trip day",
            p.day
        ));
    }
    let (main_en, main_zh, date, day_type) = labels_of(&conn, &plan_id, &dest, p.day).await?;
    let have_main_labels = nonempty(&main_en) && nonempty(&main_zh);
    if !have_main_labels && p.main_label.is_none() {
        return Err(format!(
            "day {} has no variant labels yet; pass --main-label <en> --main-label-zh <zh> \
             to name the original plan",
            p.day
        ));
    }

    let n = days::next_day_number(&conn, &plan_id, &dest).await?;
    // Allocation guarantee: a number handed out again must never inherit an old map.
    crate::map_objects::purge_day_maps(&plan_id, n).await?;

    let now_db = now_db_datetime();
    if let (Some(en), Some(zh)) = (&p.main_label, &p.main_label_zh) {
        set_labels(&conn, &plan_id, &dest, p.day, Some(en), Some(zh), &now_db).await?;
    }
    conn.execute(
        "INSERT INTO days (plan_id, destination, day_number, date, theme, day_type, status, \
                           updated_at, variant_of, variant_label, variant_label_zh) \
         VALUES (?1, ?2, ?3, ?4, NULL, ?5, 'draft', ?6, ?7, ?8, ?9)",
        params![
            plan_id.clone(),
            dest.clone(),
            n,
            date,
            day_type,
            now_db.clone(),
            p.day,
            label.to_string(),
            label_zh.to_string()
        ],
    )
    .await
    .map_err(|e| format!("days INSERT failed: {e}"))?;
    for sess in ["morning", "noon", "afternoon", "evening"] {
        conn.execute(
            "INSERT INTO timesofday (plan_id, destination, day_number, session_type, focus, \
                                     transit_notes, booking_notes, updated_at) \
             VALUES (?1, ?2, ?3, ?4, NULL, NULL, NULL, ?5)",
            params![plan_id.clone(), dest.clone(), n, sess, now_db.clone()],
        )
        .await
        .map_err(|e| format!("timesofday INSERT failed: {e}"))?;
    }

    audit(
        &conn,
        &plan_id,
        &dest,
        "day_variant_added",
        "add-day-variant",
        &format!("D{} variant D{n}", p.day),
        &[
            ("main_day", p.day.to_string()),
            ("variant_day", n.to_string()),
            ("label", label.to_string()),
            ("label_zh", label_zh.to_string()),
        ],
    )
    .await?;
    println!(
        "variant day {n} of day {} created — write it with the usual commands on day {n}",
        p.day
    );
    Ok(())
}

pub async fn run_label(args: &[String], plan_id: String) -> Result<(), String> {
    let p = parse(args, LABEL_USAGE)?;
    if p.main_label.is_some() || p.main_label_zh.is_some() {
        return Err("unknown argument: --main-label (use --label/--label-zh on the day itself)".into());
    }
    let label = p.label.as_deref().ok_or("--label is required")?;
    let label_zh = p.label_zh.as_deref().ok_or("--label-zh is required")?;
    check_label_en(label)?;
    check_label_zh(label_zh)?;

    let conn = crate::db::connect_write().await?;
    let dest = resolve_active_destination(&conn, &plan_id, p.dest.as_deref()).await?;
    labels_of(&conn, &plan_id, &dest, p.day).await?; // fail loud if the day is missing
    set_labels(&conn, &plan_id, &dest, p.day, Some(label), Some(label_zh), &now_db_datetime())
        .await?;
    audit(
        &conn,
        &plan_id,
        &dest,
        "day_variant_label_set",
        "set-day-variant-label",
        &format!("D{}", p.day),
        &[
            ("day", p.day.to_string()),
            ("label", label.to_string()),
            ("label_zh", label_zh.to_string()),
        ],
    )
    .await?;
    println!("day {} labels set: {label} / {label_zh}", p.day);
    Ok(())
}

pub async fn run_remove(args: &[String], plan_id: String) -> Result<(), String> {
    let p = parse(args, REMOVE_USAGE)?;
    if p.label.is_some() || p.label_zh.is_some() || p.main_label.is_some() || p.main_label_zh.is_some()
    {
        return Err("remove-day-variant takes no label flags".into());
    }
    let conn = crate::db::connect_write().await?;
    let dest = resolve_active_destination(&conn, &plan_id, p.dest.as_deref()).await?;
    let main = days::variant_parent(&conn, &plan_id, &dest, p.day)
        .await?
        .ok_or_else(|| format!("day {} is a trip day, not a variant; refusing to remove it", p.day))?;

    // R2 first: a failure aborts with nothing deleted.
    crate::map_objects::purge_day_maps(&plan_id, p.day).await?;

    let (pl, de, d) = (plan_id.clone(), dest.clone(), p.day);
    conn.execute(
        "DELETE FROM activity_tags WHERE activity_id IN \
            (SELECT id FROM activities WHERE plan_id = ?1 AND destination = ?2 AND day_number = ?3)",
        params![pl.clone(), de.clone(), d],
    )
    .await
    .map_err(|e| format!("activity_tags delete failed: {e}"))?;
    for tbl in [
        "activities",
        "session_meals",
        "session_activities_zh",
        "timesofday",
        "day_route_segments",
        "day_landmarks",
        "days",
    ] {
        conn.execute(
            &format!("DELETE FROM {tbl} WHERE plan_id = ?1 AND destination = ?2 AND day_number = ?3"),
            params![pl.clone(), de.clone(), d],
        )
        .await
        .map_err(|e| format!("{tbl} delete failed: {e}"))?;
    }
    let cleared = days::variants_of(&conn, &plan_id, &dest, main).await?.is_empty();
    if cleared {
        set_labels(&conn, &plan_id, &dest, main, None, None, &now_db_datetime()).await?;
    }
    let trip_id = plan_id.replace('-', "_");
    let n = crate::sync_bookings::resync_plan_bookings(&conn, &plan_id, &trip_id).await?;
    println!("bookings re-synced ({n})");
    audit(
        &conn,
        &plan_id,
        &dest,
        "day_variant_removed",
        "remove-day-variant",
        &format!("D{} variant of D{main}", p.day),
        &[
            ("main_day", main.to_string()),
            ("variant_day", p.day.to_string()),
            ("main_labels_cleared", cleared.to_string()),
        ],
    )
    .await?;
    println!("variant day {} of day {main} removed", p.day);
    Ok(())
}
