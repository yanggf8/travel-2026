//! Live-schema discovery for plan-wide passes (`db cleanup-deleted`, `copy-plan`).
//!
//! Every list here is read from `sqlite_master` + `pragma_table_info` /
//! `pragma_foreign_key_list` at runtime, never hardcoded — a hand-kept table list
//! drifts out of sync with the ~130-table schema (the same lesson as the tests'
//! `common::teardown_plan`).

use libsql::Connection;
use std::collections::BTreeMap;

/// One row of `pragma_table_info`: column name, declared type, and its 1-based
/// position in the PRIMARY KEY (0 = not part of the PK).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnInfo {
    pub name: String,
    pub decl_type: String,
    pub pk: i64,
}

impl ColumnInfo {
    pub fn new(name: &str, decl_type: &str, pk: i64) -> Self {
        ColumnInfo {
            name: name.to_string(),
            decl_type: decl_type.to_string(),
            pk,
        }
    }
}

/// Every table carrying a `plan_id` column, sorted by name.
pub async fn plan_scoped_tables(conn: &Connection) -> Result<Vec<String>, String> {
    let mut rows = conn
        .query(
            "SELECT m.name FROM sqlite_master m \
             WHERE m.type = 'table' AND m.name NOT LIKE 'sqlite_%' \
               AND EXISTS (SELECT 1 FROM pragma_table_info(m.name) p WHERE p.name = 'plan_id') \
             ORDER BY m.name",
            (),
        )
        .await
        .map_err(|e| format!("plan-scoped table discovery failed: {e}"))?;
    let mut out = Vec::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("plan-scoped table row read failed: {e}"))?
    {
        let name: String = row.get(0).map_err(|e| e.to_string())?;
        if !name.is_empty() {
            out.push(name);
        }
    }
    Ok(out)
}

/// Columns of every user table, keyed by table name, in declaration order.
/// One round trip (sqlite_master × pragma_table_info).
pub async fn all_table_columns(
    conn: &Connection,
) -> Result<BTreeMap<String, Vec<ColumnInfo>>, String> {
    let mut rows = conn
        .query(
            "SELECT m.name, p.name, p.type, p.pk \
             FROM sqlite_master m, pragma_table_info(m.name) p \
             WHERE m.type = 'table' AND m.name NOT LIKE 'sqlite_%' \
             ORDER BY m.name, p.cid",
            (),
        )
        .await
        .map_err(|e| format!("column discovery failed: {e}"))?;
    let mut out: BTreeMap<String, Vec<ColumnInfo>> = BTreeMap::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("column discovery row read failed: {e}"))?
    {
        let table: String = row.get(0).map_err(|e| e.to_string())?;
        let name: String = row.get(1).map_err(|e| e.to_string())?;
        let decl_type: String = row.get::<Option<String>>(2).ok().flatten().unwrap_or_default();
        let pk: i64 = row.get(3).unwrap_or(0);
        out.entry(table).or_default().push(ColumnInfo { name, decl_type, pk });
    }
    Ok(out)
}

/// Every foreign-key edge as `(child_table, parent_table)`, deduplicated.
pub async fn foreign_key_edges(conn: &Connection) -> Result<Vec<(String, String)>, String> {
    let mut rows = conn
        .query(
            "SELECT DISTINCT m.name, f.\"table\" \
             FROM sqlite_master m, pragma_foreign_key_list(m.name) f \
             WHERE m.type = 'table' AND m.name NOT LIKE 'sqlite_%' \
             ORDER BY m.name, f.\"table\"",
            (),
        )
        .await
        .map_err(|e| format!("foreign-key discovery failed: {e}"))?;
    let mut out = Vec::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("foreign-key row read failed: {e}"))?
    {
        let child: String = row.get(0).map_err(|e| e.to_string())?;
        let parent: String = row.get(1).map_err(|e| e.to_string())?;
        out.push((child, parent));
    }
    Ok(out)
}
