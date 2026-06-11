// `travel db exec "<sql>"` — execute one or more ad-hoc SQL statements
// against Turso. Ports scripts/turso-exec.ts. WRITE-capable (uses turso-util
// write-tier token).
//
// Differences from the TS original (intentional, per the migration plan's
// "plain-text CLI output" rule):
//   - Row output is plain `col: value, col: value` per line instead of JSON
//     objects. The TS JSON form was a developer-tooling quirk; the migrated
//     CLI produces clean text that pipelines / grep can consume.
//   - Per-statement error prefix is the same `[i/N] Error: ...`.
//   - Single-statement success: same `N row(s) affected` / per-row plain text.

use crate::db;
use libsql::Value;
use std::process;

/// Split a SQL blob on statement-terminating semicolons. Single-quoted string
/// literals are respected (`'` toggles in/out of a literal; `''` for an escaped
/// quote is handled naturally by the toggle). This is intentionally
/// minimal — db:exec is for short ad-hoc SQL, not full script parsing.
fn split_statements(sql: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut buf = String::new();
    let mut in_str = false;
    for c in sql.chars() {
        if c == '\'' {
            in_str = !in_str;
            buf.push(c);
            continue;
        }
        if c == ';' && !in_str {
            let stmt = buf.trim().to_string();
            if !stmt.is_empty() {
                out.push(stmt + ";");
            }
            buf.clear();
            continue;
        }
        buf.push(c);
    }
    let tail = buf.trim();
    if !tail.is_empty() {
        out.push(tail.to_string());
    }
    out
}

/// True if the statement's first keyword (case-insensitive) is a DML mutation
/// (DELETE / UPDATE / INSERT). Used to decide whether a 0-row result deserves a
/// loud "nothing changed" warning. DDL (CREATE/ALTER/DROP) and SELECT-likes are
/// intentionally excluded — only row-level mutations carry the silent-no-op
/// footgun this guards against.
fn is_mutation(stmt: &str) -> bool {
    let head = stmt
        .trim_start()
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_ascii_uppercase();
    matches!(head.as_str(), "DELETE" | "UPDATE" | "INSERT")
}

fn value_to_string(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::Integer(n) => n.to_string(),
        Value::Real(f) => f.to_string(),
        Value::Text(s) => s.clone(),
        Value::Blob(b) => format!("<blob {} bytes>", b.len()),
    }
}

pub async fn run(args: &[String]) -> Result<(), String> {
    let sql = args.join(" ");
    if sql.trim().is_empty() {
        eprintln!("Usage: travel db exec <SQL>");
        eprintln!("  travel db exec \"SELECT * FROM plan_metadata\"");
        eprintln!(
            "  travel db exec \"DELETE FROM a WHERE x=1; DELETE FROM b WHERE x=1;\""
        );
        process::exit(1);
    }

    let stmts = split_statements(&sql);
    if stmts.is_empty() {
        return Err("no SQL statements parsed from input".to_string());
    }

    let conn = db::connect_write().await?;

    // Single statement → keep the historical terse output (no [1/1] noise).
    if stmts.len() == 1 {
        let stmt = &stmts[0];
        match exec_one(&conn, stmt).await {
            Ok(result) => {
                // A mutating statement (DELETE/UPDATE/INSERT and any other
                // non-row-returning statement) ALWAYS reports its affected
                // count — including `0 row(s) affected` — so an agent can never
                // mistake a silent no-op for success. Row-returning SELECTs skip
                // the count line and print rows (or "ok"-equivalent emptiness).
                if !result.returns_rows {
                    println!("{} row(s) affected", result.affected_row_count);
                    if result.affected_row_count == 0 && is_mutation(stmt) {
                        eprintln!("⚠ no rows matched — nothing changed");
                    }
                }
                if !result.rows.is_empty() {
                    print_result(None, &result);
                }
            }
            Err(err) => {
                eprintln!("Error: {err}");
                process::exit(1);
            }
        }
        return Ok(());
    }

    // Multi-statement: per-statement isolation so a single failure does not
    // swallow the remaining statements. Matches the TS behavior.
    let mut any_failed = false;
    let total = stmts.len();
    for (i, stmt) in stmts.iter().enumerate() {
        let label = format!("[{}/{}]", i + 1, total);
        match exec_one(&conn, stmt).await {
            Ok(result) => {
                let affected = result.affected_row_count;
                let row_count = result.rows.len();
                if result.returns_rows {
                    if row_count > 0 {
                        println!("{} {} row(s) returned", label, row_count);
                    } else {
                        println!("{} ok", label);
                    }
                } else {
                    // Mutating statement: ALWAYS show the count, incl. 0, and
                    // warn loudly on a 0-row DELETE/UPDATE/INSERT.
                    println!("{} {} row(s) affected", label, affected);
                    if affected == 0 && is_mutation(stmt) {
                        eprintln!("{} ⚠ no rows matched — nothing changed", label);
                    }
                }
                if row_count > 0 {
                    print_result(Some(&label), &result);
                }
            }
            Err(err) => {
                any_failed = true;
                eprintln!("{} Error: {}", label, err);
            }
        }
    }
    if any_failed {
        process::exit(1);
    }
    Ok(())
}

struct ExecResult {
    /// True for SELECT / PRAGMA / WITH / RETURNING — statements run via
    /// `query()` that yield rows. False for execute()-path statements
    /// (DML + DDL), whose `affected_row_count` is meaningful and always
    /// reported.
    returns_rows: bool,
    affected_row_count: u64,
    cols: Vec<String>,
    rows: Vec<Vec<String>>,
}

async fn exec_one(conn: &libsql::Connection, stmt: &str) -> Result<ExecResult, String> {
    // Heuristic: SELECT / PRAGMA / WITH (CTE) return rows; everything else
    // uses execute() to surface affected_row_count. The leading-whitespace
    // trim and case-insensitive prefix match match the common SQL forms.
    let trimmed = stmt.trim_start();
    let head = trimmed
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_ascii_uppercase();
    let returns_rows = matches!(head.as_str(), "SELECT" | "PRAGMA" | "WITH" | "RETURNING");

    if !returns_rows {
        let affected = conn
            .execute(stmt, ())
            .await
            .map_err(|err| format!("{err}"))?;
        return Ok(ExecResult {
            returns_rows: false,
            affected_row_count: affected,
            cols: Vec::new(),
            rows: Vec::new(),
        });
    }

    let mut rows = conn
        .query(stmt, ())
        .await
        .map_err(|err| format!("{err}"))?;
    let mut cols: Vec<String> = Vec::new();
    let mut collected: Vec<Vec<String>> = Vec::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|err| format!("{err}"))?
    {
        if cols.is_empty() {
            let n = row.column_count();
            for i in 0..n {
                cols.push(
                    row.column_name(i)
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| format!("col{i}")),
                );
            }
        }
        let mut out = Vec::with_capacity(cols.len());
        for i in 0..cols.len() {
            let v = row
                .get_value(i as i32)
                .map(|v| value_to_string(&v))
                .unwrap_or_default();
            out.push(v);
        }
        collected.push(out);
    }
    Ok(ExecResult {
        returns_rows: true,
        affected_row_count: 0,
        cols,
        rows: collected,
    })
}

fn print_result(label: Option<&str>, result: &ExecResult) {
    let prefix = label.map(|l| format!("{l} ")).unwrap_or_default();
    if !result.rows.is_empty() {
        for row_vals in &result.rows {
            let parts: Vec<String> = result
                .cols
                .iter()
                .zip(row_vals.iter())
                .map(|(c, v)| format!("{}: {}", c, v))
                .collect();
            println!("{prefix}{}", parts.join(", "));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_mutation_detects_dml_keywords() {
        assert!(is_mutation("DELETE FROM plans WHERE plan_id='x'"));
        assert!(is_mutation("delete from plans"));
        assert!(is_mutation("  Update plans SET version=1"));
        assert!(is_mutation("INSERT INTO plans VALUES (1)"));
        // leading whitespace + lowercase still detected
        assert!(is_mutation("\n\t  insert into a values (1)"));
    }

    #[test]
    fn is_mutation_excludes_non_dml() {
        assert!(!is_mutation("SELECT * FROM plans"));
        assert!(!is_mutation("PRAGMA table_info(plans)"));
        assert!(!is_mutation("CREATE TABLE x (a INT)"));
        assert!(!is_mutation("ALTER TABLE x ADD COLUMN b TEXT"));
        assert!(!is_mutation("DROP TABLE x"));
        assert!(!is_mutation("WITH t AS (SELECT 1) SELECT * FROM t"));
        assert!(!is_mutation(""));
    }
}
