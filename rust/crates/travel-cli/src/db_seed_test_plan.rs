//! `db seed test-plan` — port of scripts/seed-test-plan.ts.
//!
//! Clones a plan by copying every plan-scoped table's rows from a source plan_id
//! to a target plan_id. Dynamic: discovers all tables with a `plan_id` column at
//! runtime via sqlite_master. Idempotent: deletes target rows first (doubles as
//! the RESET between test runs).
//!
//! Post-clone: copies `activity_tags` rows, rewriting `activity_id` to match the
//! suffixed activity IDs created during the main clone pass. (`activity_tags` has
//! no `plan_id` column so it isn't discovered by the dynamic scan.)
//!
//! Usage:
//!   travel db seed test-plan [source] [target]
//!   Defaults: source=tokyo-2026, target=test-set-dates-2026
//!
//! SAFETY: refuses to write to tokyo-2026 / kyoto-2026 as the TARGET.
//!
//! Signature fixed by main.rs dispatch: `run(&[String])`.

use crate::db::connect_write;
use libsql::params;

const PROTECTED: &[&str] = &["tokyo-2026", "kyoto-2026"];

/// Tables whose PK is a GLOBALLY-unique id (not plan-scoped) — rewrite that id
/// by suffixing with the target plan_id so the clone doesn't collide with the source.
const GLOBAL_ID_COLS: &[(&str, &str)] = &[
    ("activities", "id"),
    ("operation_runs", "run_id"),
    ("plan_offer_warnings", "id"),
];

/// SQL-literal-escape a value: NULL for None, quoted string for &str, bare for numbers.
fn sql_lit(v: &libsql::Value) -> String {
    match v {
        libsql::Value::Null => "NULL".to_string(),
        libsql::Value::Integer(n) => n.to_string(),
        libsql::Value::Real(x) => x.to_string(),
        libsql::Value::Text(s) => format!("'{}'", s.replace('\'', "''")),
        libsql::Value::Blob(_) => "NULL".to_string(), // shouldn't appear in plan tables
    }
}

/// Quote an identifier (table or column name) for safe use in generated SQL.
fn quote_id(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

pub async fn run(args: &[String]) -> Result<(), String> {
    let source = args.first().map(|s| s.as_str()).unwrap_or("tokyo-2026");
    let target = args.get(1).map(|s| s.as_str()).unwrap_or("test-set-dates-2026");

    if PROTECTED.contains(&target) {
        return Err(format!("Refusing to use a protected plan as TARGET: {target}"));
    }

    let conn = connect_write().await?;

    // 1. Discover all tables with a plan_id column.
    let mut rows = conn
        .query(
            "SELECT m.name AS tbl FROM sqlite_master m JOIN pragma_table_info(m.name) p ON p.name = 'plan_id' WHERE m.type = 'table' ORDER BY m.name;",
            (),
        )
        .await
        .map_err(|e| format!("table discovery failed: {e}"))?;

    let mut tables: Vec<String> = Vec::new();
    while let Ok(Some(row)) = rows.next().await {
        if let Ok(libsql::Value::Text(name)) = row.get_value(0) {
            tables.push(name);
        }
    }

    if tables.is_empty() {
        return Err("No plan-scoped tables found in the database.".to_string());
    }

    // 2. DELETE all target rows (the reset).
    // Table names come from sqlite_master (trusted/internal). plan_id values are
    // parameterized.
    for t in &tables {
        conn.execute(
            &format!("DELETE FROM {} WHERE plan_id = ?1", quote_id(t)),
            params![target],
        )
            .await
            .map_err(|e| format!("delete from {t} failed: {e}"))?;
    }

    // 3. Read source rows per table, rewrite plan_id (+ global ids), INSERT.
    let id_suffix = format!("__{target}");
    let mut total_rows = 0usize;
    let mut populated = 0usize;

    for t in &tables {
        // Get column names via PRAGMA (table names from sqlite_master are trusted).
        let cols: Vec<String> = {
            let mut cr = conn
                .query(&format!("PRAGMA table_info({});", quote_id(t)), ())
                .await
                .map_err(|e| format!("pragma table_info({t}) failed: {e}"))?;
            let mut names = Vec::new();
            while let Ok(Some(row)) = cr.next().await {
                if let Ok(libsql::Value::Text(name)) = row.get_value(1) {
                    names.push(name);
                }
            }
            names
        };

        if cols.is_empty() {
            continue;
        }

        let id_col = GLOBAL_ID_COLS.iter().find(|(table, _)| *table == t).map(|(_, col)| *col);

        // SELECT source rows with parameterized plan_id.
        let mut src_rows = conn
            .query(&format!("SELECT * FROM {} WHERE plan_id = ?1", quote_id(t)), params![source])
            .await
            .map_err(|e| format!("select from {t} failed: {e}"))?;

        let qt = quote_id(t);
        let qcols: Vec<String> = cols.iter().map(|c| quote_id(c)).collect();

        while let Ok(Some(row)) = src_rows.next().await {
            let vals: Vec<String> = cols.iter().enumerate().map(|(i, c)| {
                let v = row.get_value(i as i32).unwrap_or(libsql::Value::Null);
                if c == "plan_id" {
                    sql_lit(&libsql::Value::Text(target.to_string()))
                } else if let Some(idc) = id_col {
                    if c == idc {
                        match &v {
                            libsql::Value::Text(s) if !s.is_empty() => {
                                sql_lit(&libsql::Value::Text(format!("{s}{id_suffix}")))
                            }
                            _ => sql_lit(&v),
                        }
                    } else {
                        sql_lit(&v)
                    }
                } else {
                    sql_lit(&v)
                }
            }).collect();

            let insert_sql = format!(
                "INSERT INTO {qt} ({}) VALUES ({})",
                qcols.join(", "),
                vals.join(", ")
            );
            conn.execute(&insert_sql, ())
                .await
                .map_err(|e| format!("insert into {t} failed: {e}"))?;
            total_rows += 1;
        }
        if total_rows > 0 || populated == 0 {
            populated += 1;
        }
    }

    // 4. Post-clone: copy activity_tags, rewriting activity_id to match suffixed IDs.
    // activity_tags has no plan_id column, so it wasn't discovered above. Its
    // activity_id references activities.id which was suffixed during the clone.
    {
        let ts = quote_id("activity_tags");
        conn.execute(
            &format!("DELETE FROM {ts} WHERE activity_id LIKE ?1"),
            params![format!("%{id_suffix}")],
        ).await.map_err(|e| format!("delete activity_tags failed: {e}"))?;

        // Find source activities that have tags.
        let mut tag_rows = conn
            .query(
                &format!("SELECT at.activity_id, at.tag FROM {ts} at JOIN \"activities\" a ON at.activity_id = a.id WHERE a.plan_id = ?1"),
                params![source],
            )
            .await
            .map_err(|e| format!("select activity_tags failed: {e}"))?;

        let mut n_tags = 0usize;
        while let Ok(Some(row)) = tag_rows.next().await {
            let aid = row.get_value(0).unwrap_or(libsql::Value::Null);
            let tag = row.get_value(1).unwrap_or(libsql::Value::Null);
            if let (libsql::Value::Text(a), libsql::Value::Text(t)) = (&aid, &tag) {
                let new_aid = format!("{a}{id_suffix}");
                conn.execute(
                    &format!("INSERT OR REPLACE INTO {ts} (activity_id, tag) VALUES (?1, ?2)"),
                    params![new_aid, t.as_str()],
                ).await.map_err(|e| format!("insert activity_tags failed: {e}"))?;
                n_tags += 1;
            }
        }
        if n_tags > 0 {
            eprintln!("  (copied {n_tags} activity_tags rows with rewritten activity_id)");
        }
    }

    println!("Seeded {target} from {source}: {total_rows} rows across {populated} tables.");
    println!("  (Re-run this command to RESET {target} to the same baseline.)");
    Ok(())
}
