use std::{
    fs,
    path::{Path, PathBuf},
};

use libsql::params;

use crate::{
    db,
    error::{Error, Result},
};

#[derive(Debug, Clone, Eq, PartialEq)]
struct Migration {
    version: i64,
    filename: String,
    path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct MigrateOptions {
    pub migrations_dir: PathBuf,
    pub schema_name: String,
    pub dry_run: bool,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct MigrateReport {
    pub schema: String,
    pub current: i64,
    pub latest: i64,
    pub pending: Vec<String>,
    pub applied: Vec<String>,
}

pub async fn run_migrations(
    url: &str,
    token: Option<&str>,
    opts: MigrateOptions,
) -> Result<MigrateReport> {
    if opts.schema_name.trim().is_empty() {
        return Err(Error::validation("schema_name must not be empty"));
    }

    let migrations = list_migrations(&opts.migrations_dir)?;
    if migrations.is_empty() {
        return Err(Error::environment(format!(
            "no numbered migration files found in {}",
            opts.migrations_dir.display()
        )));
    }
    let latest = migrations
        .last()
        .map(|migration| migration.version)
        .unwrap_or(0);

    let database = db::database(url, token).await?;
    let conn = database
        .connect()
        .map_err(|err| Error::turso(err.to_string()))?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS schema_version (
            name TEXT PRIMARY KEY,
            version INTEGER NOT NULL
        )",
        params![],
    )
    .await
    .map_err(|err| Error::turso(err.to_string()))?;

    let current = current_version(&conn, &opts.schema_name).await?;
    let pending_migrations: Vec<&Migration> = migrations
        .iter()
        .filter(|migration| migration.version > current)
        .collect();
    let pending: Vec<String> = pending_migrations
        .iter()
        .map(|migration| migration.filename.clone())
        .collect();

    if opts.dry_run {
        return Ok(MigrateReport {
            schema: opts.schema_name,
            current,
            latest,
            pending,
            applied: Vec::new(),
        });
    }

    let mut applied = Vec::new();
    for migration in pending_migrations {
        apply_migration(&conn, &opts.schema_name, migration).await?;
        applied.push(migration.filename.clone());
    }

    Ok(MigrateReport {
        schema: opts.schema_name,
        current,
        latest,
        pending: Vec::new(),
        applied,
    })
}

fn list_migrations(dir: &Path) -> Result<Vec<Migration>> {
    let entries = fs::read_dir(dir).map_err(|err| {
        Error::environment(format!(
            "cannot read migrations directory {}: {err}",
            dir.display()
        ))
    })?;

    let mut migrations = Vec::new();
    for entry in entries {
        let entry = entry.map_err(Error::internal)?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        let Some(filename) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        let Some(version) = migration_version(filename) else {
            continue;
        };

        migrations.push(Migration {
            version,
            filename: filename.to_string(),
            path,
        });
    }

    migrations.sort_by_key(|migration| migration.version);
    Ok(migrations)
}

fn migration_version(filename: &str) -> Option<i64> {
    let (prefix, rest) = filename.split_once('_')?;
    if !rest.ends_with(".sql") || prefix.is_empty() {
        return None;
    }
    prefix.parse::<i64>().ok()
}

async fn current_version(conn: &libsql::Connection, name: &str) -> Result<i64> {
    let mut rows = conn
        .query(
            "SELECT version FROM schema_version WHERE name = ?",
            params![name],
        )
        .await
        .map_err(|err| Error::turso(err.to_string()))?;

    let Some(row) = rows
        .next()
        .await
        .map_err(|err| Error::turso(err.to_string()))?
    else {
        return Ok(0);
    };

    row.get::<i64>(0)
        .map_err(|err| Error::turso(err.to_string()))
}

async fn apply_migration(
    conn: &libsql::Connection,
    schema_name: &str,
    migration: &Migration,
) -> Result<()> {
    let sql = fs::read_to_string(&migration.path).map_err(|err| {
        Error::environment(format!(
            "cannot read migration {}: {err}",
            migration.filename
        ))
    })?;

    begin(conn).await?;
    let result = async {
        for statement in split_sql(&sql) {
            conn.execute(&statement, params![])
                .await
                .map_err(|err| Error::turso(format!("{}: {err}", migration.filename)))?;
        }

        conn.execute(
            "INSERT INTO schema_version (name, version) VALUES (?, ?)
             ON CONFLICT(name) DO UPDATE SET version = MAX(version, excluded.version)",
            params![schema_name, migration.version],
        )
        .await
        .map_err(|err| Error::turso(err.to_string()))?;

        Ok(())
    }
    .await;
    finish(conn, result).await
}

async fn begin(conn: &libsql::Connection) -> Result<()> {
    conn.execute("BEGIN", params![])
        .await
        .map_err(|err| Error::turso(err.to_string()))?;
    Ok(())
}

async fn finish(conn: &libsql::Connection, result: Result<()>) -> Result<()> {
    match result {
        Ok(()) => {
            conn.execute("COMMIT", params![])
                .await
                .map_err(|err| Error::turso(err.to_string()))?;
            Ok(())
        }
        Err(err) => {
            let _ = conn.execute("ROLLBACK", params![]).await;
            Err(err)
        }
    }
}

/// Split a SQL file into individual statements on top-level `;` boundaries.
fn split_sql(sql: &str) -> Vec<String> {
    enum State {
        Normal,
        SingleQuote,
        DoubleQuote,
        LineComment,
        BlockComment,
    }

    let mut statements = Vec::new();
    let mut start = 0;
    let mut state = State::Normal;
    let bytes = sql.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        let ch = bytes[i];
        let next = bytes.get(i + 1).copied();

        match state {
            State::Normal => match ch {
                b'\'' => {
                    state = State::SingleQuote;
                    i += 1;
                }
                b'"' => {
                    state = State::DoubleQuote;
                    i += 1;
                }
                b'-' if next == Some(b'-') => {
                    state = State::LineComment;
                    i += 2;
                }
                b'/' if next == Some(b'*') => {
                    state = State::BlockComment;
                    i += 2;
                }
                b';' => {
                    let statement = sql[start..i].trim();
                    if !statement.is_empty() {
                        statements.push(statement.to_string());
                    }
                    start = i + 1;
                    i += 1;
                }
                _ => i += 1,
            },
            State::SingleQuote => {
                if ch == b'\'' && next == Some(b'\'') {
                    i += 2;
                } else if ch == b'\'' {
                    state = State::Normal;
                    i += 1;
                } else {
                    i += 1;
                }
            }
            State::DoubleQuote => {
                if ch == b'"' && next == Some(b'"') {
                    i += 2;
                } else if ch == b'"' {
                    state = State::Normal;
                    i += 1;
                } else {
                    i += 1;
                }
            }
            State::LineComment => {
                if ch == b'\n' {
                    state = State::Normal;
                }
                i += 1;
            }
            State::BlockComment => {
                if ch == b'*' && next == Some(b'/') {
                    state = State::Normal;
                    i += 2;
                } else {
                    i += 1;
                }
            }
        }
    }

    let tail = sql[start..].trim();
    if !tail.is_empty() {
        statements.push(tail.to_string());
    }

    statements
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn parses_numbered_migration_filenames() {
        assert_eq!(migration_version("001_persona.sql"), Some(1));
        assert_eq!(migration_version("010_add_thing.sql"), Some(10));
        assert_eq!(migration_version("runner.ts"), None);
        assert_eq!(migration_version("001.sql"), None);
        assert_eq!(migration_version("abc_name.sql"), None);
    }

    #[test]
    fn splits_sql_without_splitting_quoted_semicolons() {
        let statements = split_sql(
            "CREATE TABLE t (v TEXT DEFAULT ';');\n\
             INSERT INTO t VALUES ('a'';b');\n\
             INSERT INTO t VALUES (\"c;d\")",
        );

        assert_eq!(
            statements,
            vec![
                "CREATE TABLE t (v TEXT DEFAULT ';')",
                "INSERT INTO t VALUES ('a'';b')",
                "INSERT INTO t VALUES (\"c;d\")",
            ]
        );
    }

    #[test]
    fn ignores_semicolons_inside_line_comments() {
        let statements = split_sql(
            "-- header note; with embedded semicolon\n\
             ALTER TABLE t ADD COLUMN c TEXT;\n\
             ALTER TABLE t ADD COLUMN d TEXT;",
        );

        assert_eq!(
            statements,
            vec![
                "-- header note; with embedded semicolon\n\
                 ALTER TABLE t ADD COLUMN c TEXT",
                "ALTER TABLE t ADD COLUMN d TEXT",
            ]
        );
    }

    #[test]
    fn ignores_semicolons_inside_block_comments() {
        let statements = split_sql(
            "/* multi-line ;\n   still in comment; */\n\
             CREATE TABLE u (id INTEGER);\n\
             CREATE TABLE v (id INTEGER);",
        );

        assert_eq!(
            statements,
            vec![
                "/* multi-line ;\n   still in comment; */\n\
                 CREATE TABLE u (id INTEGER)",
                "CREATE TABLE v (id INTEGER)",
            ]
        );
    }

    #[test]
    fn ignores_trailing_line_comment_with_semicolon() {
        let statements = split_sql(
            "ALTER TABLE t ADD COLUMN x TEXT; -- trailing note; with ;\n\
             ALTER TABLE t ADD COLUMN y TEXT;",
        );

        assert_eq!(
            statements,
            vec![
                "ALTER TABLE t ADD COLUMN x TEXT",
                "-- trailing note; with ;\n\
                 ALTER TABLE t ADD COLUMN y TEXT",
            ]
        );
    }

    #[test]
    fn comment_markers_inside_quoted_strings_are_not_comments() {
        let statements = split_sql(
            "INSERT INTO t VALUES ('-- not a comment; still in string');\n\
             INSERT INTO t VALUES ('/* nor this; */');",
        );

        assert_eq!(
            statements,
            vec![
                "INSERT INTO t VALUES ('-- not a comment; still in string')",
                "INSERT INTO t VALUES ('/* nor this; */')",
            ]
        );
    }

    #[test]
    fn semicolon_inside_string_does_not_split() {
        let statements = split_sql("INSERT INTO t VALUES ('a;b;c')");
        assert_eq!(statements, vec!["INSERT INTO t VALUES ('a;b;c')"]);
    }

    #[test]
    fn lists_migrations_sorted_and_ignores_non_migrations() {
        let dir = temp_dir("turso-util-migrate-test");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("002_second.sql"), "-- two").unwrap();
        fs::write(dir.join("001_first.sql"), "-- one").unwrap();
        fs::write(dir.join("runner.ts"), "ignored").unwrap();

        let migrations = list_migrations(&dir).unwrap();

        assert_eq!(
            migrations
                .iter()
                .map(|migration| migration.filename.as_str())
                .collect::<Vec<_>>(),
            vec!["001_first.sql", "002_second.sql"]
        );

        fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn dry_run_fills_pending_not_applied() {
        let dir = temp_dir("tu-mig");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("001_a.sql"), "CREATE TABLE a (id INTEGER);").unwrap();
        let db_path = dir.join("t.db");
        let report = run_migrations(
            &format!("file:{}", db_path.display()),
            None,
            MigrateOptions {
                migrations_dir: dir.clone(),
                schema_name: "test".into(),
                dry_run: true,
            },
        )
        .await
        .unwrap();
        assert_eq!(report.pending, vec!["001_a.sql".to_string()]);
        assert!(report.applied.is_empty());
        let _ = fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn real_run_fills_applied_not_pending() {
        let dir = temp_dir("tu-mig-real");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("001_a.sql"), "CREATE TABLE a (id INTEGER);").unwrap();
        let db_path = dir.join("t.db");
        let report = run_migrations(
            &format!("file:{}", db_path.display()),
            None,
            MigrateOptions {
                migrations_dir: dir.clone(),
                schema_name: "test".into(),
                dry_run: false,
            },
        )
        .await
        .unwrap();
        assert!(report.pending.is_empty());
        assert_eq!(report.applied, vec!["001_a.sql".to_string()]);
        let _ = fs::remove_dir_all(dir);
    }

    fn temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("{prefix}-{nanos}"))
    }
}
