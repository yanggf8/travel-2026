// `travel db status` — quick read-only snapshot of common Turso table counts +
// last-write timestamps. Ports scripts/turso-status.ts. Read-only, plain text.
//
// Output format (must match TS byte-for-byte for parity, except for the
// naturally-volatile timestamp values):
//
//   offers_count: N
//   events_count: N
//   destinations_count: N
//   bookings_count: N
//   booking_events_count: N
//   offers_last_scraped_at: <ts or empty>
//   events_last_created_at: <ts or empty>
//
// On per-query error: `<label>: (error) <message>` — same as TS.

use crate::db;

struct CountQuery {
    label: &'static str,
    sql: &'static str,
}

const QUERIES: &[CountQuery] = &[
    CountQuery {
        label: "offers_count",
        sql: "SELECT COUNT(*) AS n FROM offers",
    },
    CountQuery {
        label: "events_count",
        sql: "SELECT COUNT(*) AS n FROM events",
    },
    CountQuery {
        label: "destinations_count",
        sql: "SELECT COUNT(*) AS n FROM destinations",
    },
    CountQuery {
        label: "bookings_count",
        sql: "SELECT COUNT(*) AS n FROM bookings_current",
    },
    CountQuery {
        label: "booking_events_count",
        sql: "SELECT COUNT(*) AS n FROM bookings_events",
    },
    CountQuery {
        label: "offers_last_scraped_at",
        sql: "SELECT MAX(COALESCE(last_seen_at, scraped_at)) AS v FROM offers",
    },
    CountQuery {
        label: "events_last_created_at",
        sql: "SELECT MAX(created_at) AS v FROM events",
    },
];

pub async fn run() -> Result<(), String> {
    let conn = db::connect_read().await?;
    for q in QUERIES {
        match run_one(&conn, q.sql).await {
            Ok(value) => println!("{}: {}", q.label, value),
            Err(err) => println!("{}: (error) {}", q.label, err),
        }
    }
    Ok(())
}

async fn run_one(conn: &libsql::Connection, sql: &str) -> Result<String, String> {
    let mut rows = conn
        .query(sql, ())
        .await
        .map_err(|err| format!("{err}"))?;
    let Some(row) = rows
        .next()
        .await
        .map_err(|err| format!("{err}"))?
    else {
        return Ok(String::new());
    };
    // TS unwraps `row[0]` for array form, or `Object.values(row)[0]` for object
    // form; libsql exposes column-by-name or column-by-index. Pick by index 0
    // which matches the SELECT * projection in all 7 TS queries.
    match row.get_value(0) {
        Ok(libsql::Value::Null) => Ok(String::new()),
        Ok(libsql::Value::Integer(n)) => Ok(n.to_string()),
        Ok(libsql::Value::Real(f)) => Ok(f.to_string()),
        Ok(libsql::Value::Text(s)) => Ok(s),
        Ok(other) => Ok(format!("{other:?}")),
        Err(_) => Ok(String::new()),
    }
}
