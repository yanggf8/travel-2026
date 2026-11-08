use std::collections::HashMap;

use libsql::{Connection, Row};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct PlanDocument {
    pub plan_id: String,
    pub doc_key: String,
    pub kind: String,
    pub label: String,
    pub label_zh: String,
    pub sort_order: i64,
    pub r2_key: String,
    pub sha256: String,
    pub size_bytes: i64,
    pub created_at: String,
    pub updated_at: String,
    pub days: Vec<i64>,
}

const COLUMNS: &str = "plan_id, doc_key, kind, label, label_zh, sort_order, r2_key, sha256, \
                       size_bytes, created_at, updated_at";

fn from_row(row: &Row) -> Result<PlanDocument, String> {
    let get = |e: libsql::Error| format!("plan_documents decode failed: {e}");
    Ok(PlanDocument {
        plan_id: row.get(0).map_err(get)?,
        doc_key: row.get(1).map_err(get)?,
        kind: row.get(2).map_err(get)?,
        label: row.get(3).map_err(get)?,
        label_zh: row.get(4).map_err(get)?,
        sort_order: row.get(5).map_err(get)?,
        r2_key: row.get(6).map_err(get)?,
        sha256: row.get(7).map_err(get)?,
        size_bytes: row.get(8).map_err(get)?,
        created_at: row.get(9).map_err(get)?,
        updated_at: row.get(10).map_err(get)?,
        days: Vec::new(),
    })
}

async fn attach_days(
    conn: &Connection,
    plan_id: &str,
    documents: &mut [PlanDocument],
) -> Result<(), String> {
    let mut rows = conn
        .query(
            "SELECT doc_key, day_number FROM plan_document_days \
             WHERE plan_id = ?1 ORDER BY doc_key, day_number",
            libsql::params![plan_id.to_string()],
        )
        .await
        .map_err(|e| format!("plan_document_days list failed: {e}"))?;
    let mut days: HashMap<String, Vec<i64>> = HashMap::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("plan_document_days row read failed: {e}"))?
    {
        days.entry(row.get::<String>(0).map_err(|e| e.to_string())?)
            .or_default()
            .push(row.get::<i64>(1).map_err(|e| e.to_string())?);
    }
    for document in documents {
        document.days = days.remove(&document.doc_key).unwrap_or_default();
    }
    Ok(())
}

pub async fn list(conn: &Connection, plan_id: &str) -> Result<Vec<PlanDocument>, String> {
    let sql = format!(
        "SELECT {COLUMNS} FROM plan_documents WHERE plan_id = ?1 \
         ORDER BY CASE kind WHEN 'flight_eticket' THEN 0 WHEN 'hotel_voucher' THEN 1 \
                            WHEN 'rail_ticket' THEN 2 WHEN 'tour_voucher' THEN 3 ELSE 4 END, \
                  sort_order, doc_key"
    );
    let mut rows = conn
        .query(&sql, libsql::params![plan_id.to_string()])
        .await
        .map_err(|e| format!("plan_documents list failed: {e}"))?;
    let mut documents = Vec::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("plan_documents row read failed: {e}"))?
    {
        documents.push(from_row(&row)?);
    }
    attach_days(conn, plan_id, &mut documents).await?;
    Ok(documents)
}

pub async fn get(
    conn: &Connection,
    plan_id: &str,
    doc_key: &str,
) -> Result<Option<PlanDocument>, String> {
    let sql = format!(
        "SELECT {COLUMNS} FROM plan_documents WHERE plan_id = ?1 AND doc_key = ?2"
    );
    let mut rows = conn
        .query(
            &sql,
            libsql::params![plan_id.to_string(), doc_key.to_string()],
        )
        .await
        .map_err(|e| format!("plan_documents get failed: {e}"))?;
    let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("plan_documents row read failed: {e}"))?
    else {
        return Ok(None);
    };
    let mut documents = vec![from_row(&row)?];
    attach_days(conn, plan_id, &mut documents).await?;
    Ok(documents.pop())
}

pub async fn upsert(
    conn: &Connection,
    document: &PlanDocument,
    now_db: &str,
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO plan_documents \
             (plan_id, doc_key, kind, label, label_zh, sort_order, r2_key, sha256, size_bytes, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10) \
         ON CONFLICT(plan_id, doc_key) DO UPDATE SET \
             kind = excluded.kind, label = excluded.label, label_zh = excluded.label_zh, \
             sort_order = excluded.sort_order, r2_key = excluded.r2_key, \
             sha256 = excluded.sha256, size_bytes = excluded.size_bytes, \
             updated_at = excluded.updated_at",
        libsql::params![
            document.plan_id.clone(),
            document.doc_key.clone(),
            document.kind.clone(),
            document.label.clone(),
            document.label_zh.clone(),
            document.sort_order,
            document.r2_key.clone(),
            document.sha256.clone(),
            document.size_bytes,
            now_db.to_string()
        ],
    )
    .await
    .map_err(|e| format!("plan_documents upsert failed: {e}"))?;
    Ok(())
}

pub async fn replace_days(
    conn: &Connection,
    plan_id: &str,
    doc_key: &str,
    days: &[i64],
) -> Result<(), String> {
    conn.execute(
        "DELETE FROM plan_document_days WHERE plan_id = ?1 AND doc_key = ?2",
        libsql::params![plan_id.to_string(), doc_key.to_string()],
    )
    .await
    .map_err(|e| format!("plan_document_days delete failed: {e}"))?;
    for day in days {
        conn.execute(
            "INSERT INTO plan_document_days (plan_id, doc_key, day_number) VALUES (?1, ?2, ?3)",
            libsql::params![plan_id.to_string(), doc_key.to_string(), *day],
        )
        .await
        .map_err(|e| format!("plan_document_days insert failed: {e}"))?;
    }
    Ok(())
}

pub async fn delete(
    conn: &Connection,
    plan_id: &str,
    doc_key: &str,
) -> Result<u64, String> {
    conn.execute(
        "DELETE FROM plan_document_days WHERE plan_id = ?1 AND doc_key = ?2",
        libsql::params![plan_id.to_string(), doc_key.to_string()],
    )
    .await
    .map_err(|e| format!("plan_document_days delete failed: {e}"))?;
    conn.execute(
        "DELETE FROM plan_documents WHERE plan_id = ?1 AND doc_key = ?2",
        libsql::params![plan_id.to_string(), doc_key.to_string()],
    )
    .await
    .map_err(|e| format!("plan_documents delete failed: {e}"))
}

pub async fn plan_exists(conn: &Connection, plan_id: &str) -> Result<bool, String> {
    let mut rows = conn
        .query(
            "SELECT 1 FROM plans WHERE plan_id = ?1 AND deleted_at IS NULL LIMIT 1",
            libsql::params![plan_id.to_string()],
        )
        .await
        .map_err(|e| format!("plan existence query failed: {e}"))?;
    Ok(rows
        .next()
        .await
        .map_err(|e| format!("plan existence row read failed: {e}"))?
        .is_some())
}

pub async fn trip_day_exists(
    conn: &Connection,
    plan_id: &str,
    day_number: i64,
) -> Result<bool, String> {
    let mut rows = conn
        .query(
            "SELECT 1 FROM days WHERE plan_id = ?1 AND day_number = ?2 AND variant_of IS NULL LIMIT 1",
            libsql::params![plan_id.to_string(), day_number],
        )
        .await
        .map_err(|e| format!("plan document day validation failed: {e}"))?;
    Ok(rows
        .next()
        .await
        .map_err(|e| format!("plan document day row read failed: {e}"))?
        .is_some())
}
