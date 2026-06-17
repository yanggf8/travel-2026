// `travel db schema [<table>]` — discover table/column names without guessing.
//
// Editing data via `db exec` repeatedly hit "no such column" errors (e.g.
// `timesofday` has `focus` not `sort_order`; `session_meals` uses `meal` not
// `text`; route stops are `from_place`/`to_place`). There was no easy way to
// list a table's columns. This command wraps `PRAGMA table_info(<table>)` (and
// `sqlite_master` for the table list) into plain-text output an agent can read.
//
//   travel db schema                 → list all tables
//   travel db schema timesofday      → list columns of `timesofday`

use crate::db;
use libsql::params;

pub async fn run(args: &[String]) -> Result<(), String> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!(
            "Usage:\n  travel db schema            (list all tables)\n  travel db schema <table>    (list columns: name, type, notnull, pk)"
        );
        return Ok(());
    }
    let conn = db::connect_read().await?;
    match args.first() {
        None => list_tables(&conn).await,
        Some(table) => list_columns(&conn, table).await,
    }
}

async fn list_tables(conn: &libsql::Connection) -> Result<(), String> {
    let mut rows = conn
        .query(
            "SELECT name FROM sqlite_master \
             WHERE type = 'table' AND name NOT LIKE 'sqlite_%' \
             ORDER BY name",
            (),
        )
        .await
        .map_err(|e| format!("sqlite_master query failed: {e}"))?;
    let mut names = Vec::new();
    while let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
        names.push(row.get::<String>(0).unwrap_or_default());
    }
    if names.is_empty() {
        return Err("no tables found".to_string());
    }
    println!("{} tables:", names.len());
    for n in &names {
        println!("  {n}");
    }
    println!("\nColumns of one table:  travel db schema <table>");
    Ok(())
}

async fn list_columns(conn: &libsql::Connection, table: &str) -> Result<(), String> {
    // PRAGMA table_info is parameterizable as table_info(?1) on libsql.
    let mut rows = conn
        .query("SELECT name, type, \"notnull\", pk FROM pragma_table_info(?1)",
            params![table.to_string()])
        .await
        .map_err(|e| format!("pragma_table_info query failed: {e}"))?;
    let mut cols = Vec::new();
    while let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
        let name: String = row.get(0).unwrap_or_default();
        let ty: String = row.get(1).unwrap_or_default();
        let notnull: i64 = row.get(2).unwrap_or(0);
        let pk: i64 = row.get(3).unwrap_or(0);
        cols.push((name, ty, notnull == 1, pk > 0));
    }
    if cols.is_empty() {
        return Err(format!(
            "no such table: {table} (run `travel db schema` to list tables)"
        ));
    }
    println!("{table} ({} columns):", cols.len());
    for (name, ty, notnull, pk) in &cols {
        let mut flags = Vec::new();
        if *pk {
            flags.push("PK");
        }
        if *notnull {
            flags.push("NOT NULL");
        }
        let suffix = if flags.is_empty() {
            String::new()
        } else {
            format!("  [{}]", flags.join(", "))
        };
        let ty_disp = if ty.is_empty() { "?" } else { ty.as_str() };
        println!("  {name}  {ty_disp}{suffix}");
    }
    Ok(())
}
