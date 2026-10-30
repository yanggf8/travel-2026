//! SQL for `copy-plan`: schema-driven, server-side `INSERT … SELECT` copies of one
//! plan's rows under a new plan id, plus the read-only planning queries
//! (row counts, id lists, booking keys) the CLI runs before it writes.
//!
//! DAL boundary: owns the SQL strings only. The transaction, the validation, the
//! fresh-id generation and the audit back half (`record_operation`) stay in
//! `travel-cli` (`copy_plan.rs`).
//!
//! Every statement is built from the LIVE column list (`plan_schema`), so a column
//! added by a later migration is copied without touching this file. Values never
//! get spliced into SQL text: ids and maps travel as bound parameters, identifiers
//! come from `sqlite_master` and are double-quoted.

use crate::repo::plan_schema::ColumnInfo;
use libsql::{Connection, Value};

/// `(old, new)` id pairs. Bound as a `VALUES` CTE inside each statement.
pub type IdMap = [(String, String)];

/// How a table's PRIMARY KEY relates to the column `copy-plan` rewrites.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PkKind {
    /// The PK contains the rewritten key column (plan_id / booking_key /
    /// activity_id): rewriting it keeps the copied rows unique.
    IncludesKey,
    /// No declared PRIMARY KEY (plain rowid table): copies cannot conflict.
    NoPk,
    /// A single `INTEGER PRIMARY KEY` (rowid alias): omit it so SQLite assigns one.
    SingleInteger(String),
    /// A single TEXT-affinity PK: every copied row needs a fresh id.
    SingleText(String),
    /// Anything else: no safe generic copy exists.
    Unsupported(Vec<String>),
}

/// Classify `columns`' PRIMARY KEY relative to `key_column`.
pub fn classify_pk(columns: &[ColumnInfo], key_column: &str) -> PkKind {
    let mut pk: Vec<&ColumnInfo> = columns.iter().filter(|c| c.pk > 0).collect();
    pk.sort_by_key(|c| c.pk);
    if pk.is_empty() {
        return PkKind::NoPk;
    }
    if pk.iter().any(|c| c.name == key_column) {
        return PkKind::IncludesKey;
    }
    if pk.len() == 1 {
        let c = pk[0];
        // Only the exact type name INTEGER makes a rowid alias that SQLite fills in.
        if c.decl_type.trim().eq_ignore_ascii_case("INTEGER") {
            return PkKind::SingleInteger(c.name.clone());
        }
        if affinity(&c.decl_type) == Affinity::Text {
            return PkKind::SingleText(c.name.clone());
        }
    }
    PkKind::Unsupported(pk.iter().map(|c| c.name.clone()).collect())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Affinity {
    Integer,
    Text,
    Blob,
    Real,
    Numeric,
}

/// SQLite's column-affinity rules (datatype3 §3.1), in their documented order.
fn affinity(decl: &str) -> Affinity {
    let d = decl.to_ascii_uppercase();
    if d.contains("INT") {
        Affinity::Integer
    } else if d.contains("CHAR") || d.contains("CLOB") || d.contains("TEXT") {
        Affinity::Text
    } else if d.contains("BLOB") || d.trim().is_empty() {
        Affinity::Blob
    } else if d.contains("REAL") || d.contains("FLOA") || d.contains("DOUB") {
        Affinity::Real
    } else {
        Affinity::Numeric
    }
}

/// True when a column can hold a text id (TEXT affinity, or untyped/BLOB which
/// stores text verbatim). Those cells get the activity-id remap.
pub fn may_hold_text_id(decl: &str) -> bool {
    matches!(affinity(decl), Affinity::Text | Affinity::Blob)
}

/// How a copy statement picks its source rows and rewrites its key column.
#[derive(Debug, Clone, Copy)]
pub enum Source<'a> {
    /// `WHERE s.plan_id = ?src`; `plan_id` is written as `dst`.
    Plan { src: &'a str, dst: &'a str },
    /// `JOIN keymap k ON k.old = s.<column>`; `<column>` is written as `k.new`.
    /// Rows whose key is not in `map` are not copied.
    KeyMap { column: &'a str, map: &'a IdMap },
}

/// One table's copy, fully described.
pub struct CopyStatement<'a> {
    pub table: &'a str,
    pub columns: &'a [ColumnInfo],
    pub source: Source<'a>,
    /// Columns written as a constant (e.g. `bookings_current.trip_id`).
    pub constants: &'a [(&'a str, &'a str)],
    /// A single INTEGER PK to leave out (SQLite assigns a new rowid).
    pub omit: Option<&'a str>,
    /// A single TEXT PK and its old→new map. Rows are JOINed to the map, so a row
    /// that appeared after the map was built is NOT copied — the caller's
    /// affected-row check turns that into a loud failure.
    pub fresh_pk: Option<(&'a str, &'a IdMap)>,
    /// Old→new activity ids, applied to every other text-capable cell whose value
    /// equals an old id exactly.
    pub actmap: &'a IdMap,
}

/// SQL text plus its positional (`?`) parameters, in textual order.
#[derive(Debug)]
pub struct Sql {
    pub sql: String,
    pub params: Vec<Value>,
}

fn ident(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

fn push_map_cte(ctes: &mut Vec<String>, params: &mut Vec<Value>, name: &str, map: &IdMap) {
    if map.is_empty() {
        return;
    }
    let rows: Vec<&str> = map.iter().map(|_| "(?, ?)").collect();
    ctes.push(format!("{name}(old, new) AS (VALUES {})", rows.join(", ")));
    for (old, new) in map {
        params.push(Value::Text(old.clone()));
        params.push(Value::Text(new.clone()));
    }
}

/// Build the `WITH … INSERT INTO t (…) SELECT … FROM t AS s …` for one table.
pub fn build_copy(st: &CopyStatement<'_>) -> Sql {
    let mut params: Vec<Value> = Vec::new();
    let mut ctes: Vec<String> = Vec::new();
    push_map_cte(&mut ctes, &mut params, "actmap", st.actmap);
    let key_column = match st.source {
        Source::Plan { .. } => "plan_id",
        Source::KeyMap { column, map } => {
            push_map_cte(&mut ctes, &mut params, "keymap", map);
            column
        }
    };
    if let Some((_, map)) = st.fresh_pk {
        push_map_cte(&mut ctes, &mut params, "pkmap", map);
    }

    let mut cols: Vec<String> = Vec::new();
    let mut exprs: Vec<String> = Vec::new();
    for c in st.columns {
        if st.omit == Some(c.name.as_str()) {
            continue;
        }
        cols.push(ident(&c.name));
        let src = format!("s.{}", ident(&c.name));
        let expr = if c.name == key_column {
            match st.source {
                Source::Plan { dst, .. } => {
                    params.push(Value::Text(dst.to_string()));
                    "?".to_string()
                }
                Source::KeyMap { .. } => "k.new".to_string(),
            }
        } else if let Some((_, v)) = st.constants.iter().find(|(n, _)| *n == c.name) {
            params.push(Value::Text((*v).to_string()));
            "?".to_string()
        } else if st.fresh_pk.map(|(n, _)| n) == Some(c.name.as_str()) {
            "p.new".to_string()
        } else if !st.actmap.is_empty() && may_hold_text_id(&c.decl_type) {
            format!("COALESCE((SELECT a.new FROM actmap a WHERE a.old = {src}), {src})")
        } else {
            src
        };
        exprs.push(expr);
    }

    let mut sql = String::new();
    if !ctes.is_empty() {
        sql.push_str("WITH ");
        sql.push_str(&ctes.join(", "));
        sql.push(' ');
    }
    sql.push_str(&format!(
        "INSERT INTO {} ({}) SELECT {} FROM {} AS s",
        ident(st.table),
        cols.join(", "),
        exprs.join(", "),
        ident(st.table)
    ));
    if let Source::KeyMap { column, .. } = st.source {
        sql.push_str(&format!(" JOIN keymap k ON k.old = s.{}", ident(column)));
    }
    if let Some((pk, _)) = st.fresh_pk {
        sql.push_str(&format!(" JOIN pkmap p ON p.old = s.{}", ident(pk)));
    }
    if let Source::Plan { src, .. } = st.source {
        sql.push_str(" WHERE s.\"plan_id\" = ?");
        params.push(Value::Text(src.to_string()));
    }
    Sql { sql, params }
}

/// Which rows of a table a count covers.
#[derive(Debug, Clone, Copy)]
pub enum Selection<'a> {
    /// `<column> = value`
    Equals { column: &'a str, value: &'a str },
    /// `substr(<column>, 1, len(prefix)) = prefix` (no LIKE: `_` is a wildcard there).
    Prefix { column: &'a str, prefix: &'a str },
    /// `<column> IN (old ids of actmap)`
    InActMap { column: &'a str },
}

/// One `(label, table, selection)` arm of a combined count query. `exclude`
/// lists the columns NOT counted as activity-id references (the rewritten key,
/// a fresh PK, …).
pub struct CountItem<'a> {
    pub label: &'a str,
    pub table: &'a str,
    pub columns: &'a [ColumnInfo],
    pub selection: Selection<'a>,
    pub exclude: &'a [&'a str],
}

/// One `SELECT label, COUNT(*), <activity-id refs> … UNION ALL …` over every
/// item — a single round trip for the whole plan. Result rows are
/// `(label, rows, refs)` in item order (see [`run_counts`]).
pub fn build_counts(items: &[CountItem<'_>], actmap: &IdMap) -> Sql {
    let mut params: Vec<Value> = Vec::new();
    let mut sql = String::new();
    if !actmap.is_empty() {
        let mut ctes = Vec::new();
        push_map_cte(&mut ctes, &mut params, "actmap", actmap);
        sql.push_str("WITH ");
        sql.push_str(&ctes.join(", "));
        sql.push(' ');
    }
    let mut arms = Vec::new();
    for (i, it) in items.iter().enumerate() {
        let refs: Vec<String> = if actmap.is_empty() {
            Vec::new()
        } else {
            it.columns
                .iter()
                .filter(|c| !it.exclude.contains(&c.name.as_str()) && may_hold_text_id(&c.decl_type))
                .map(|c| {
                    let col = format!("s.{}", ident(&c.name));
                    format!("({col} IS NOT NULL AND {col} IN (SELECT old FROM actmap))")
                })
                .collect()
        };
        let refs_expr = if refs.is_empty() {
            "0".to_string()
        } else {
            format!("COALESCE(SUM({}), 0)", refs.join(" + "))
        };
        params.push(Value::Text(it.label.to_string()));
        let (where_sql, wparams): (String, Vec<Value>) = match it.selection {
            Selection::Equals { column, value } => (
                format!("s.{} = ?", ident(column)),
                vec![Value::Text(value.to_string())],
            ),
            Selection::Prefix { column, prefix } => (
                format!("substr(s.{}, 1, ?) = ?", ident(column)),
                vec![
                    Value::Integer(prefix.chars().count() as i64),
                    Value::Text(prefix.to_string()),
                ],
            ),
            Selection::InActMap { column } => {
                if actmap.is_empty() {
                    ("0".to_string(), Vec::new())
                } else {
                    (format!("s.{} IN (SELECT old FROM actmap)", ident(column)), Vec::new())
                }
            }
        };
        params.extend(wparams);
        arms.push(format!(
            "SELECT ? AS label, {i} AS ord, COUNT(*) AS n, {refs_expr} AS refs FROM {} AS s WHERE {where_sql}",
            ident(it.table)
        ));
    }
    sql.push_str(&format!(
        "SELECT label, n, refs FROM ({}) ORDER BY ord",
        arms.join(" UNION ALL ")
    ));
    Sql { sql, params }
}

/// Arms per combined count query. Turso rejects long compound SELECTs ("too many
/// terms in compound SELECT"), so [`count`] splits the items into chunks.
pub const COUNT_ARMS_PER_QUERY: usize = 25;

/// Count every item → `(label, rows, activity-id refs)` in item order, in
/// `ceil(items / COUNT_ARMS_PER_QUERY)` round trips.
pub async fn count(
    conn: &Connection,
    items: &[CountItem<'_>],
    actmap: &IdMap,
) -> Result<Vec<(String, i64, i64)>, String> {
    let mut out = Vec::new();
    for chunk in items.chunks(COUNT_ARMS_PER_QUERY) {
        out.extend(run_counts(conn, &build_counts(chunk, actmap)).await?);
    }
    Ok(out)
}

/// Run one [`build_counts`] query → `(label, rows, activity-id refs)`.
pub async fn run_counts(conn: &Connection, q: &Sql) -> Result<Vec<(String, i64, i64)>, String> {
    if q.sql.is_empty() {
        return Ok(Vec::new());
    }
    let mut rows = conn
        .query(&q.sql, q.params.clone())
        .await
        .map_err(|e| format!("count query failed: {e}"))?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().await.map_err(|e| format!("count row read failed: {e}"))? {
        let label: String = row.get(0).map_err(|e| e.to_string())?;
        let n: i64 = row.get(1).unwrap_or(0);
        let refs: i64 = row.get(2).unwrap_or(0);
        out.push((label, n, refs));
    }
    Ok(out)
}

/// `SELECT <column> FROM <table> WHERE plan_id = ?` — the old ids of a
/// single-TEXT-PK table, sorted.
pub async fn plan_column_values(
    conn: &Connection,
    table: &str,
    column: &str,
    plan_id: &str,
) -> Result<Vec<String>, String> {
    let sql = format!(
        "SELECT {c} FROM {t} WHERE \"plan_id\" = ?1 ORDER BY {c}",
        c = ident(column),
        t = ident(table)
    );
    let mut rows = conn
        .query(&sql, libsql::params![plan_id.to_string()])
        .await
        .map_err(|e| format!("{table}.{column} read failed: {e}"))?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().await.map_err(|e| format!("{table} row read failed: {e}"))? {
        let v: Option<String> = row.get(0).ok();
        match v {
            Some(v) => out.push(v),
            None => return Err(format!("{table}.{column} has a NULL/non-text primary key for plan {plan_id}")),
        }
    }
    Ok(out)
}

/// Distinct `<column>` values starting with `prefix` across `tables`, sorted.
pub async fn prefixed_values(
    conn: &Connection,
    tables: &[String],
    column: &str,
    prefix: &str,
) -> Result<Vec<String>, String> {
    if tables.is_empty() {
        return Ok(Vec::new());
    }
    let mut params: Vec<Value> = Vec::new();
    let arms: Vec<String> = tables
        .iter()
        .map(|t| {
            params.push(Value::Integer(prefix.chars().count() as i64));
            params.push(Value::Text(prefix.to_string()));
            format!(
                "SELECT {c} AS v FROM {t} WHERE substr({c}, 1, ?) = ?",
                c = ident(column),
                t = ident(t)
            )
        })
        .collect();
    let sql = format!("{} ORDER BY v", arms.join(" UNION "));
    let mut rows = conn
        .query(&sql, params)
        .await
        .map_err(|e| format!("{column} prefix query failed: {e}"))?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().await.map_err(|e| format!("{column} row read failed: {e}"))? {
        out.push(row.get::<String>(0).map_err(|e| e.to_string())?);
    }
    Ok(out)
}

/// Rows of `table` whose `trip_column` and `key_column` prefix disagree:
/// `trip = ? AND key does not start with prefix`, or the reverse.
pub async fn trip_key_mismatches(
    conn: &Connection,
    table: &str,
    key_column: &str,
    trip_column: &str,
    trip: &str,
    prefix: &str,
) -> Result<i64, String> {
    let sql = format!(
        "SELECT COUNT(*) FROM {t} WHERE ({tc} = ?1 AND substr({kc}, 1, ?2) <> ?3) \
            OR (substr({kc}, 1, ?2) = ?3 AND {tc} <> ?1)",
        t = ident(table),
        tc = ident(trip_column),
        kc = ident(key_column)
    );
    let mut rows = conn
        .query(
            &sql,
            libsql::params![trip.to_string(), prefix.chars().count() as i64, prefix.to_string()],
        )
        .await
        .map_err(|e| format!("{table} trip/key check failed: {e}"))?;
    match rows.next().await.map_err(|e| e.to_string())? {
        Some(row) => Ok(row.get::<i64>(0).unwrap_or(0)),
        None => Ok(0),
    }
}

/// `(version, deleted_at IS NOT NULL)` of a plan, or `None` if it isn't in `plans`.
pub async fn plan_version(conn: &Connection, plan_id: &str) -> Result<Option<(i64, bool)>, String> {
    let mut rows = conn
        .query(
            "SELECT version, deleted_at IS NOT NULL AND deleted_at <> '' FROM plans WHERE plan_id = ?1",
            libsql::params![plan_id.to_string()],
        )
        .await
        .map_err(|e| format!("plans query failed: {e}"))?;
    match rows.next().await.map_err(|e| format!("plans row read failed: {e}"))? {
        Some(row) => {
            let version: i64 = row.get(0).unwrap_or(0);
            let deleted: i64 = row.get(1).unwrap_or(0);
            Ok(Some((version, deleted != 0)))
        }
        None => Ok(None),
    }
}

/// Plan ids (deleted ones included) whose booking trip id — plan_id with
/// '-' -> '_' — equals `trip`, sorted.
pub async fn plans_with_trip_id(conn: &Connection, trip: &str) -> Result<Vec<String>, String> {
    let mut rows = conn
        .query(
            "SELECT plan_id FROM plans WHERE replace(plan_id, '-', '_') = ?1 ORDER BY plan_id",
            libsql::params![trip.to_string()],
        )
        .await
        .map_err(|e| format!("trip id owner query failed: {e}"))?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().await.map_err(|e| format!("trip id owner row read failed: {e}"))? {
        out.push(row.get::<String>(0).map_err(|e| e.to_string())?);
    }
    Ok(out)
}

/// Execute one built statement; returns the affected-row count.
pub async fn execute(conn: &Connection, q: &Sql) -> Result<u64, String> {
    conn.execute(&q.sql, q.params.clone())
        .await
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cols(spec: &[(&str, &str, i64)]) -> Vec<ColumnInfo> {
        spec.iter().map(|(n, t, pk)| ColumnInfo::new(n, t, *pk)).collect()
    }

    #[test]
    fn classify_pk_covers_every_shape() {
        let days = cols(&[("plan_id", "TEXT", 1), ("destination", "TEXT", 2), ("day_number", "INTEGER", 3)]);
        assert_eq!(classify_pk(&days, "plan_id"), PkKind::IncludesKey);

        let activities = cols(&[("id", "TEXT", 1), ("plan_id", "TEXT", 0), ("title", "TEXT", 0)]);
        assert_eq!(classify_pk(&activities, "plan_id"), PkKind::SingleText("id".into()));

        let warnings = cols(&[("id", "INTEGER", 1), ("plan_id", "TEXT", 0)]);
        assert_eq!(classify_pk(&warnings, "plan_id"), PkKind::SingleInteger("id".into()));

        let rowid = cols(&[("plan_id", "TEXT", 0), ("note", "TEXT", 0)]);
        assert_eq!(classify_pk(&rowid, "plan_id"), PkKind::NoPk);

        // BIGINT is integer affinity but NOT a rowid alias → no auto-assigned id.
        let bigint = cols(&[("id", "BIGINT", 1), ("plan_id", "TEXT", 0)]);
        assert_eq!(classify_pk(&bigint, "plan_id"), PkKind::Unsupported(vec!["id".into()]));

        let composite = cols(&[("a", "TEXT", 1), ("b", "TEXT", 2), ("plan_id", "TEXT", 0)]);
        assert_eq!(
            classify_pk(&composite, "plan_id"),
            PkKind::Unsupported(vec!["a".into(), "b".into()])
        );

        // Booking tables are classified against booking_key, not plan_id.
        let payload = cols(&[("booking_key", "TEXT", 1), ("sort_order", "INTEGER", 2)]);
        assert_eq!(classify_pk(&payload, "booking_key"), PkKind::IncludesKey);
    }

    #[test]
    fn text_id_affinity() {
        assert!(may_hold_text_id("TEXT"));
        assert!(may_hold_text_id("VARCHAR(40)"));
        assert!(may_hold_text_id(""));
        assert!(!may_hold_text_id("INTEGER"));
        assert!(!may_hold_text_id("DATETIME"));
        assert!(!may_hold_text_id("REAL"));
    }

    fn text(v: &Value) -> String {
        match v {
            Value::Text(s) => s.clone(),
            Value::Integer(i) => i.to_string(),
            other => format!("{other:?}"),
        }
    }

    #[test]
    fn build_copy_plan_table_with_fresh_text_pk_and_remap() {
        let c = cols(&[("id", "TEXT", 1), ("plan_id", "TEXT", 0), ("title", "TEXT", 0), ("day_number", "INTEGER", 0)]);
        let act: Vec<(String, String)> = vec![("a1".into(), "b1".into())];
        let q = build_copy(&CopyStatement {
            table: "activities",
            columns: &c,
            source: Source::Plan { src: "src-p", dst: "dst-p" },
            constants: &[],
            omit: None,
            fresh_pk: Some(("id", &act)),
            actmap: &act,
        });
        assert_eq!(
            q.sql,
            "WITH actmap(old, new) AS (VALUES (?, ?)), pkmap(old, new) AS (VALUES (?, ?)) \
             INSERT INTO \"activities\" (\"id\", \"plan_id\", \"title\", \"day_number\") \
             SELECT p.new, ?, COALESCE((SELECT a.new FROM actmap a WHERE a.old = s.\"title\"), s.\"title\"), s.\"day_number\" \
             FROM \"activities\" AS s JOIN pkmap p ON p.old = s.\"id\" WHERE s.\"plan_id\" = ?"
        );
        let p: Vec<String> = q.params.iter().map(text).collect();
        assert_eq!(p, vec!["a1", "b1", "a1", "b1", "dst-p", "src-p"]);
    }

    #[test]
    fn build_copy_omits_integer_pk_and_skips_remap_when_no_activities() {
        let c = cols(&[("id", "INTEGER", 1), ("plan_id", "TEXT", 0), ("message", "TEXT", 0)]);
        let q = build_copy(&CopyStatement {
            table: "plan_offer_warnings",
            columns: &c,
            source: Source::Plan { src: "s", dst: "d" },
            constants: &[],
            omit: Some("id"),
            fresh_pk: None,
            actmap: &[],
        });
        assert_eq!(
            q.sql,
            "INSERT INTO \"plan_offer_warnings\" (\"plan_id\", \"message\") \
             SELECT ?, s.\"message\" FROM \"plan_offer_warnings\" AS s WHERE s.\"plan_id\" = ?"
        );
        let p: Vec<String> = q.params.iter().map(text).collect();
        assert_eq!(p, vec!["d", "s"]);
    }

    #[test]
    fn build_copy_keymap_with_constant() {
        let c = cols(&[("booking_key", "TEXT", 1), ("trip_id", "TEXT", 0), ("title", "TEXT", 0)]);
        let km: Vec<(String, String)> = vec![("old:k".into(), "new:k".into())];
        let q = build_copy(&CopyStatement {
            table: "bookings_current",
            columns: &c,
            source: Source::KeyMap { column: "booking_key", map: &km },
            constants: &[("trip_id", "new")],
            omit: None,
            fresh_pk: None,
            actmap: &[],
        });
        assert_eq!(
            q.sql,
            "WITH keymap(old, new) AS (VALUES (?, ?)) \
             INSERT INTO \"bookings_current\" (\"booking_key\", \"trip_id\", \"title\") \
             SELECT k.new, ?, s.\"title\" FROM \"bookings_current\" AS s JOIN keymap k ON k.old = s.\"booking_key\""
        );
        let p: Vec<String> = q.params.iter().map(text).collect();
        assert_eq!(p, vec!["old:k", "new:k", "new"]);
    }

    #[test]
    fn build_counts_unions_arms_in_order() {
        let c = cols(&[("plan_id", "TEXT", 1), ("value", "TEXT", 0), ("n", "INTEGER", 0)]);
        let act: Vec<(String, String)> = vec![("a1".into(), "b1".into())];
        let q = build_counts(
            &[
                CountItem { label: "t1", table: "t1", columns: &c, selection: Selection::Equals { column: "plan_id", value: "p" }, exclude: &["plan_id"] },
                CountItem { label: "t2", table: "t2", columns: &c, selection: Selection::Prefix { column: "value", prefix: "p_x:" }, exclude: &["value"] },
            ],
            &act,
        );
        assert!(q.sql.starts_with("WITH actmap(old, new) AS (VALUES (?, ?)) SELECT label, n, refs FROM ("));
        assert!(q.sql.contains("(s.\"value\" IS NOT NULL AND s.\"value\" IN (SELECT old FROM actmap))"));
        assert!(q.sql.contains("substr(s.\"value\", 1, ?) = ?"));
        assert!(q.sql.ends_with(") ORDER BY ord"));
        let p: Vec<String> = q.params.iter().map(text).collect();
        assert_eq!(p, vec!["a1", "b1", "t1", "p", "t2", "4", "p_x:"]);
    }
}
