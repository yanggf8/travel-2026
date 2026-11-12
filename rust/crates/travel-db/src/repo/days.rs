//! `days`-table domain access shared by the itinerary mutations (`set-day-theme`, route segments,
//! …). DAL boundary: owns the `days` domain SQL; the audit triad stays in `travel-cli`.

use libsql::Connection;

/// True if a `days` row exists for `(plan_id, destination, day)`. Mutations require the parent day
/// to pre-exist (scaffolded separately) so a `completed` audit always implies a row actually
/// changed — callers fail loud when this is false.
pub async fn exists(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
) -> Result<bool, String> {
    let mut rows = conn
        .query(
            "SELECT 1 FROM days WHERE plan_id = ?1 AND destination = ?2 AND day_number = ?3",
            libsql::params![plan_id.to_string(), destination.to_string(), day],
        )
        .await
        .map_err(|e| format!("days existence query failed: {e}"))?;
    Ok(rows
        .next()
        .await
        .map_err(|e| format!("days existence row read failed: {e}"))?
        .is_some())
}

/// Day-variant parent: `Ok(None)` = a trip day (`variant_of IS NULL`), `Ok(Some(main))` = a variant
/// of day `main`. `Err` when the day does not exist (fail loud, never a silent "trip day").
pub async fn variant_parent(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
) -> Result<Option<i64>, String> {
    let mut rows = conn
        .query(
            "SELECT variant_of FROM days WHERE plan_id = ?1 AND destination = ?2 AND day_number = ?3",
            libsql::params![plan_id.to_string(), destination.to_string(), day],
        )
        .await
        .map_err(|e| format!("days variant_of query failed: {e}"))?;
    match rows
        .next()
        .await
        .map_err(|e| format!("days variant_of row read failed: {e}"))?
    {
        Some(row) => row
            .get::<Option<i64>>(0)
            .map_err(|e| format!("days variant_of decode failed: {e}")),
        None => Err(format!(
            "day {day} does not exist for plan {plan_id} ({destination})"
        )),
    }
}

/// Day numbers of the variants of `main_day`, ascending.
pub async fn variants_of(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    main_day: i64,
) -> Result<Vec<i64>, String> {
    let mut rows = conn
        .query(
            "SELECT day_number FROM days WHERE plan_id = ?1 AND destination = ?2 \
             AND variant_of = ?3 ORDER BY day_number",
            libsql::params![plan_id.to_string(), destination.to_string(), main_day],
        )
        .await
        .map_err(|e| format!("days variants query failed: {e}"))?;
    let mut out = Vec::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("days variants row read failed: {e}"))?
    {
        out.push(row.get::<i64>(0).map_err(|e| format!("days variants decode failed: {e}"))?);
    }
    Ok(out)
}

/// Every day variant of a plan (optionally one destination): variant day_number -> (main day,
/// EN label). The label is `None` only on an invariant violation validate already blocks.
pub async fn variant_map(
    conn: &Connection,
    plan_id: &str,
    destination: Option<&str>,
) -> Result<std::collections::HashMap<i64, (i64, Option<String>)>, String> {
    let mut rows = conn
        .query(
            "SELECT day_number, variant_of, variant_label FROM days WHERE plan_id = ?1 \
             AND variant_of IS NOT NULL AND (?2 IS NULL OR destination = ?2)",
            libsql::params![plan_id.to_string(), destination.map(str::to_string)],
        )
        .await
        .map_err(|e| format!("days variant map query failed: {e}"))?;
    let mut out = std::collections::HashMap::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("days variant map row read failed: {e}"))?
    {
        out.insert(
            row.get::<i64>(0).map_err(|e| format!("days variant map day decode failed: {e}"))?,
            (
                row.get::<i64>(1).map_err(|e| format!("days variant map parent decode failed: {e}"))?,
                row.get::<Option<String>>(2)
                    .map_err(|e| format!("days variant map label decode failed: {e}"))?,
            ),
        );
    }
    Ok(out)
}

/// Every day variant's (main day, EN label, ZH label), optionally for one destination. Labels are
/// `None` only on an invariant violation validate already blocks.
pub async fn variant_label_rows(
    conn: &Connection,
    plan_id: &str,
    destination: Option<&str>,
) -> Result<Vec<(i64, Option<String>, Option<String>)>, String> {
    let mut rows = conn
        .query(
            "SELECT variant_of, variant_label, variant_label_zh FROM days WHERE plan_id = ?1 \
             AND variant_of IS NOT NULL AND (?2 IS NULL OR destination = ?2) ORDER BY day_number",
            libsql::params![plan_id.to_string(), destination.map(str::to_string)],
        )
        .await
        .map_err(|e| format!("days variant labels query failed: {e}"))?;
    let mut out = Vec::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("days variant labels row read failed: {e}"))?
    {
        let g = |e: libsql::Error| format!("days variant labels decode failed: {e}");
        out.push((
            row.get::<i64>(0).map_err(g)?,
            row.get::<Option<String>>(1).map_err(g)?,
            row.get::<Option<String>>(2).map_err(g)?,
        ));
    }
    Ok(out)
}

/// The earliest MAIN-day date of a plan (`days.date`, variants excluded); None when no main day
/// has a date. `travel verify-site` pins the dashboard clock (`?at=`) to the day before it, so
/// its page checks see the page outside the trip window (no 現在・接下來 card, every block open).
pub async fn first_main_day_date(conn: &Connection, plan_id: &str) -> Result<Option<String>, String> {
    let mut rows = conn
        .query(
            "SELECT MIN(date) FROM days WHERE plan_id = ?1 AND variant_of IS NULL \
             AND date IS NOT NULL AND date <> ''",
            libsql::params![plan_id.to_string()],
        )
        .await
        .map_err(|e| format!("days first-date query failed: {e}"))?;
    match rows.next().await.map_err(|e| format!("days first-date row read failed: {e}"))? {
        Some(row) => row.get::<Option<String>>(0).map_err(|e| format!("days first-date decode failed: {e}")),
        None => Ok(None),
    }
}

/// Next free day_number for plan+destination: `MAX(day_number)+1` over trip days AND variants
/// (1 when the plan has no days yet).
pub async fn next_day_number(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
) -> Result<i64, String> {
    let mut rows = conn
        .query(
            "SELECT COALESCE(MAX(day_number), 0) + 1 FROM days WHERE plan_id = ?1 AND destination = ?2",
            libsql::params![plan_id.to_string(), destination.to_string()],
        )
        .await
        .map_err(|e| format!("days next number query failed: {e}"))?;
    match rows
        .next()
        .await
        .map_err(|e| format!("days next number row read failed: {e}"))?
    {
        Some(row) => row
            .get::<i64>(0)
            .map_err(|e| format!("days next number decode failed: {e}")),
        None => Ok(1),
    }
}

/// (variant_label, variant_label_zh) of one day row (both NULL when unset).
pub async fn variant_labels(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
) -> Result<(Option<String>, Option<String>), String> {
    let mut rows = conn
        .query(
            "SELECT variant_label, variant_label_zh FROM days \
             WHERE plan_id = ?1 AND destination = ?2 AND day_number = ?3",
            libsql::params![plan_id.to_string(), destination.to_string(), day],
        )
        .await
        .map_err(|e| format!("days labels query failed: {e}"))?;
    match rows
        .next()
        .await
        .map_err(|e| format!("days labels row read failed: {e}"))?
    {
        Some(row) => Ok((
            row.get::<Option<String>>(0).map_err(|e| format!("days label decode failed: {e}"))?,
            row.get::<Option<String>>(1).map_err(|e| format!("days label_zh decode failed: {e}"))?,
        )),
        None => Err(format!("day {day} does not exist for plan {plan_id} ({destination})")),
    }
}

/// Overwrite both variant labels of a day (NULL clears); no `updated_at` bump (swap-days touches separately).
pub async fn set_variant_labels(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    label: &Option<String>,
    label_zh: &Option<String>,
) -> Result<(), String> {
    conn.execute(
        "UPDATE days SET variant_label = ?1, variant_label_zh = ?2 \
         WHERE plan_id = ?3 AND destination = ?4 AND day_number = ?5",
        libsql::params![
            label.clone(),
            label_zh.clone(),
            plan_id.to_string(),
            destination.to_string(),
            day
        ],
    )
    .await
    .map_err(|e| format!("days label update failed: {e}"))?;
    Ok(())
}

/// Re-point the variant `variant_day` at `new_main`, copying the new main's `date`/`day_type`.
pub async fn repoint_variant(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    variant_day: i64,
    new_main: i64,
    date: &Option<String>,
    day_type: &Option<String>,
    now_db: &str,
) -> Result<(), String> {
    conn.execute(
        "UPDATE days SET variant_of = ?1, date = ?2, day_type = ?3, updated_at = ?4 \
         WHERE plan_id = ?5 AND destination = ?6 AND day_number = ?7",
        libsql::params![
            new_main,
            date.clone(),
            day_type.clone(),
            now_db.to_string(),
            plan_id.to_string(),
            destination.to_string(),
            variant_day
        ],
    )
    .await
    .map_err(|e| format!("days variant repoint failed: {e}"))?;
    Ok(())
}

/// (date, day_type) of one day row.
pub async fn date_and_type(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
) -> Result<(Option<String>, Option<String>), String> {
    let mut rows = conn
        .query(
            "SELECT date, day_type FROM days WHERE plan_id = ?1 AND destination = ?2 AND day_number = ?3",
            libsql::params![plan_id.to_string(), destination.to_string(), day],
        )
        .await
        .map_err(|e| format!("days date query failed: {e}"))?;
    match rows.next().await.map_err(|e| format!("days date row read failed: {e}"))? {
        Some(row) => Ok((
            row.get::<Option<String>>(0).map_err(|e| format!("days date decode failed: {e}"))?,
            row.get::<Option<String>>(1).map_err(|e| format!("days day_type decode failed: {e}"))?,
        )),
        None => Err(format!("day {day} does not exist for plan {plan_id} ({destination})")),
    }
}

/// Apply a `set-day-theme` write to the `days` row, always bumping `updated_at` to `now_db`:
/// - both `theme` and `theme_zh` present → set both
/// - only `theme` → set theme
/// - only `theme_zh` → set theme_zh
/// - neither → touch `updated_at` only (touchItinerary)
///
/// Bound params only; the SQL variant is chosen by which fields are `Some` (mirrors the prior
/// inline 4-way conditional exactly).
pub async fn set_theme(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    theme: Option<&str>,
    theme_zh: Option<&str>,
    now_db: &str,
) -> Result<(), String> {
    match (theme, theme_zh) {
        (Some(t), Some(tz)) => {
            conn.execute(
                "UPDATE days SET theme = ?1, theme_zh = ?2, updated_at = ?3 \
                 WHERE plan_id = ?4 AND destination = ?5 AND day_number = ?6",
                libsql::params![
                    t.to_string(),
                    tz.to_string(),
                    now_db.to_string(),
                    plan_id.to_string(),
                    destination.to_string(),
                    day
                ],
            )
            .await
            .map_err(|e| format!("days UPDATE (theme+zh) failed: {e}"))?;
        }
        (Some(t), None) => {
            conn.execute(
                "UPDATE days SET theme = ?1, updated_at = ?2 \
                 WHERE plan_id = ?3 AND destination = ?4 AND day_number = ?5",
                libsql::params![
                    t.to_string(),
                    now_db.to_string(),
                    plan_id.to_string(),
                    destination.to_string(),
                    day
                ],
            )
            .await
            .map_err(|e| format!("days UPDATE (theme) failed: {e}"))?;
        }
        (None, Some(tz)) => {
            conn.execute(
                "UPDATE days SET theme_zh = ?1, updated_at = ?2 \
                 WHERE plan_id = ?3 AND destination = ?4 AND day_number = ?5",
                libsql::params![
                    tz.to_string(),
                    now_db.to_string(),
                    plan_id.to_string(),
                    destination.to_string(),
                    day
                ],
            )
            .await
            .map_err(|e| format!("days UPDATE (theme_zh) failed: {e}"))?;
        }
        (None, None) => {
            conn.execute(
                "UPDATE days SET updated_at = ?1 \
                 WHERE plan_id = ?2 AND destination = ?3 AND day_number = ?4",
                libsql::params![
                    now_db.to_string(),
                    plan_id.to_string(),
                    destination.to_string(),
                    day
                ],
            )
            .await
            .map_err(|e| format!("days UPDATE (touch) failed: {e}"))?;
        }
    }
    Ok(())
}

/// Insert a variant `days` row (status `draft`, `variant_of = main`) plus its four empty
/// `timesofday` sessions. Writes only; the caller owns the transaction and the audit triad.
pub async fn insert_variant_day(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    date: &Option<String>,
    day_type: &Option<String>,
    main_day: i64,
    label: &str,
    label_zh: &str,
    now_db: &str,
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO days (plan_id, destination, day_number, date, theme, day_type, status, \
                           updated_at, variant_of, variant_label, variant_label_zh) \
         VALUES (?1, ?2, ?3, ?4, NULL, ?5, 'draft', ?6, ?7, ?8, ?9)",
        libsql::params![
            plan_id.to_string(),
            destination.to_string(),
            day,
            date.clone(),
            day_type.clone(),
            now_db.to_string(),
            main_day,
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
            libsql::params![
                plan_id.to_string(),
                destination.to_string(),
                day,
                sess,
                now_db.to_string()
            ],
        )
        .await
        .map_err(|e| format!("timesofday INSERT failed: {e}"))?;
    }
    Ok(())
}

/// Delete one day row and every child row it owns (activity_tags first, `days` last).
pub async fn delete_day_content(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
) -> Result<(), String> {
    conn.execute(
        "DELETE FROM activity_tags WHERE activity_id IN \
            (SELECT id FROM activities WHERE plan_id = ?1 AND destination = ?2 AND day_number = ?3)",
        libsql::params![plan_id.to_string(), destination.to_string(), day],
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
            libsql::params![plan_id.to_string(), destination.to_string(), day],
        )
        .await
        .map_err(|e| format!("{tbl} delete failed: {e}"))?;
    }
    Ok(())
}
