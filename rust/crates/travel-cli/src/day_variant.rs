// `travel add-day-variant` / `set-day-variant-label` / `remove-day-variant` — alternative plans
// for one trip day (spec 2026-11-08-day-variants). A variant is a `days` row whose `variant_of`
// points at its main day; it owns the usual child rows (timesofday, activities, ...).
//
// Every command writes the audit triad: plan_events (+ plan_event_data), plans.version bump and
// an operation_runs row (via cascade::common::record_operation).

use libsql::{Connection, params};
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
) -> Result<
    (
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    ),
    String,
> {
    let (en, zh) = days::variant_labels(conn, plan, dest, day).await?;
    let (date, day_type) = days::date_and_type(conn, plan, dest, day).await?;
    Ok((en, zh, date, day_type))
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
    days::set_variant_labels(
        conn,
        plan,
        dest,
        day,
        &en.map(str::to_string),
        &zh.map(str::to_string),
    )
    .await?;
    travel_db::repo::itinerary::touch_day(conn, plan, dest, day, now_db).await
}

/// Run `body` inside BEGIN/COMMIT on `conn`; ROLLBACK (error ignored) and return the Err otherwise.
async fn in_tx<T, F>(conn: &Connection, name: &str, body: F) -> Result<T, String>
where
    F: std::future::Future<Output = Result<T, String>>,
{
    conn.execute("BEGIN", params![])
        .await
        .map_err(|e| format!("{name} BEGIN failed: {e}"))?;
    match body.await {
        Ok(v) => {
            conn.execute("COMMIT", params![])
                .await
                .map_err(|e| format!("{name} COMMIT failed: {e}"))?;
            Ok(v)
        }
        Err(e) => {
            let _ = conn.execute("ROLLBACK", params![]).await;
            Err(e)
        }
    }
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
    insert_event(
        conn,
        plan,
        "dest_process",
        dest,
        PROCESS,
        dp,
        event,
        &now_iso,
        None,
        None,
    )
    .await?;
    insert_kv_rows(conn, plan, "dest_process", dest, PROCESS, dp, kv).await?;
    insert_event(
        conn, plan, "timeline", "", PROCESS, tl, event, &now_iso, None, None,
    )
    .await?;
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

    if days::variant_parent(&conn, &plan_id, &dest, p.day)
        .await?
        .is_some()
    {
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
    in_tx(&conn, "add-day-variant", async {
        if let (Some(en), Some(zh)) = (&p.main_label, &p.main_label_zh) {
            set_labels(&conn, &plan_id, &dest, p.day, Some(en), Some(zh), &now_db).await?;
        }
        days::insert_variant_day(
            &conn, &plan_id, &dest, n, &date, &day_type, p.day, label, label_zh, &now_db,
        )
        .await?;
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
        .await
    })
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
        return Err(
            "unknown argument: --main-label (use --label/--label-zh on the day itself)".into(),
        );
    }
    let label = p.label.as_deref().ok_or("--label is required")?;
    let label_zh = p.label_zh.as_deref().ok_or("--label-zh is required")?;
    check_label_en(label)?;
    check_label_zh(label_zh)?;

    let conn = crate::db::connect_write().await?;
    let dest = resolve_active_destination(&conn, &plan_id, p.dest.as_deref()).await?;
    labels_of(&conn, &plan_id, &dest, p.day).await?; // fail loud if the day is missing
    in_tx(&conn, "set-day-variant-label", async {
        set_labels(
            &conn,
            &plan_id,
            &dest,
            p.day,
            Some(label),
            Some(label_zh),
            &now_db_datetime(),
        )
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
        .await
    })
    .await?;
    println!("day {} labels set: {label} / {label_zh}", p.day);
    Ok(())
}

pub async fn run_remove(args: &[String], plan_id: String) -> Result<(), String> {
    let p = parse(args, REMOVE_USAGE)?;
    if p.label.is_some()
        || p.label_zh.is_some()
        || p.main_label.is_some()
        || p.main_label_zh.is_some()
    {
        return Err("remove-day-variant takes no label flags".into());
    }
    let conn = crate::db::connect_write().await?;
    let dest = resolve_active_destination(&conn, &plan_id, p.dest.as_deref()).await?;
    let main = days::variant_parent(&conn, &plan_id, &dest, p.day)
        .await?
        .ok_or_else(|| {
            format!(
                "day {} is a trip day, not a variant; refusing to remove it",
                p.day
            )
        })?;

    // R2 first: a failure aborts with nothing deleted.
    crate::map_objects::purge_day_maps(&plan_id, p.day).await?;

    let n = in_tx(&conn, "remove-day-variant", async {
        days::delete_day_content(&conn, &plan_id, &dest, p.day).await?;
        let cleared = days::variants_of(&conn, &plan_id, &dest, main)
            .await?
            .is_empty();
        if cleared {
            set_labels(&conn, &plan_id, &dest, main, None, None, &now_db_datetime()).await?;
        }
        let trip_id = plan_id.replace('-', "_");
        let n = crate::sync_bookings::resync_plan_bookings(&conn, &plan_id, &trip_id).await?;
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
        Ok(n)
    })
    .await?;
    println!("bookings re-synced ({n})");
    println!("variant day {} of day {main} removed", p.day);
    Ok(())
}
