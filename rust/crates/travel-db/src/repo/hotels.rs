//! `hotels` + `hotel_access_lines` domain writes for `set-hotel`.
//!
//! DAL boundary: owns the hotels upsert + access-line replace. The audit triad
//! (`plan_events`/`plan_event_data`/`operation_runs`/`plans.version`) stays in `travel-cli`
//! (`cascade::common`).

use libsql::Connection;

/// The fields a `set-hotel` write may set. Only `Some`/non-empty fields are written; the
/// `(plan_id, destination)` upsert always bumps `updated_at`.
#[derive(Debug, Clone, Default)]
pub struct HotelWrite {
    pub name: Option<String>,
    pub check_in: Option<String>,
    pub notes: Option<String>,
    /// Access lines; when non-empty they REPLACE all existing rows (DELETE-then-reinsert).
    pub access: Vec<String>,
}

/// UPSERT the `hotels` row (only the provided columns) and, when `access` is non-empty, replace
/// the `hotel_access_lines` rows. Always runs the upsert so the parent row exists and the access
/// lines are never orphaned. Bound params throughout (column names are fixed literals).
pub async fn upsert(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    input: &HotelWrite,
    now_db: &str,
) -> Result<(), String> {
    upsert_hotel_row(conn, plan_id, destination, input, now_db).await?;

    if !input.access.is_empty() {
        conn.execute(
            "DELETE FROM hotel_access_lines WHERE plan_id = ?1 AND destination = ?2",
            libsql::params![plan_id.to_string(), destination.to_string()],
        )
        .await
        .map_err(|e| format!("hotel_access_lines DELETE failed: {e}"))?;
        for (i, line) in input.access.iter().enumerate() {
            conn.execute(
                "INSERT INTO hotel_access_lines (plan_id, destination, sort_order, line, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                libsql::params![
                    plan_id.to_string(),
                    destination.to_string(),
                    i as i64,
                    line.clone(),
                    now_db.to_string()
                ],
            )
            .await
            .map_err(|e| format!("hotel_access_lines INSERT failed: {e}"))?;
        }
    }
    Ok(())
}

async fn upsert_hotel_row(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    input: &HotelWrite,
    now_db: &str,
) -> Result<(), String> {
    // (column, value) pairs the user provided, plus updated_at. Column names are fixed string
    // literals (never user input) — only ?N placeholders and bound values vary.
    let mut cols: Vec<&str> = Vec::new();
    let mut vals: Vec<String> = Vec::new();
    if let Some(n) = &input.name {
        cols.push("name");
        vals.push(n.clone());
    }
    if let Some(c) = &input.check_in {
        cols.push("check_in");
        vals.push(c.clone());
    }
    if let Some(no) = &input.notes {
        cols.push("notes");
        vals.push(no.clone());
    }
    cols.push("updated_at");
    vals.push(now_db.to_string());

    // INSERT columns = PK (plan_id, destination) + provided columns.
    let mut insert_cols: Vec<String> = vec!["plan_id".into(), "destination".into()];
    insert_cols.extend(cols.iter().map(|c| c.to_string()));

    let mut params: Vec<libsql::Value> = vec![
        libsql::Value::Text(plan_id.to_string()),
        libsql::Value::Text(destination.to_string()),
    ];
    params.extend(vals.iter().map(|v| libsql::Value::Text(v.clone())));

    let insert_ph: Vec<String> = (1..=insert_cols.len()).map(|n| format!("?{n}")).collect();

    // DO UPDATE SET re-applies the provided columns with a fresh copy of the values, numbered
    // after the INSERT params.
    let base = params.len();
    let update_sets: Vec<String> = cols
        .iter()
        .enumerate()
        .map(|(idx, c)| format!("{c} = ?{}", base + 1 + idx))
        .collect();
    params.extend(vals.iter().map(|v| libsql::Value::Text(v.clone())));

    let sql = format!(
        "INSERT INTO hotels ({}) VALUES ({}) \
         ON CONFLICT(plan_id, destination) DO UPDATE SET {}",
        insert_cols.join(", "),
        insert_ph.join(", "),
        update_sets.join(", "),
    );
    conn.execute(&sql, params)
        .await
        .map_err(|e| format!("hotels upsert failed: {e}"))?;
    Ok(())
}
