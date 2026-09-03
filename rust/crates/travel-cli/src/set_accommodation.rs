// `travel set-accommodation --hotel <name> --room-type <type> --price <twd> [--date YYYY-MM-DD] [--dest <slug>] [--plan-id <id>]`
// — domestic Taiwan accommodation booking (independent of Japan `set-hotel` / `set-flight`).
//
// Steps:
//   1. validate --hotel / --room-type / --price (+ --date shape)
//   2. resolve dest via `cascade::common::resolve_active_destination` (fail loud)
//   3. verify `domestic_accommodations` contains the hotel+room_type row (hint query-accommodation)
//   4. INSERT/UPSERT `bookings_current` (category=accommodation, title="Hotel Room", status=booked, price_twd)
//   5. advance `process_statuses` P4 (process_4_accommodation) to `booked` via shortest-legal hops
//      (emit_status_changed per hop + record_operation once)
//   6. plain-text output: ✅ Booked accommodation: <hotel> <room> TWD <price> for <dest> plan <plan_id>

use crate::cascade::common::{
    emit_status_changed, now_db_datetime, now_rfc3339, read_version, record_operation,
    resolve_active_destination, validate_transition,
};
use std::collections::{HashSet, VecDeque};

#[derive(Debug)]
struct Args {
    hotel: String,
    room_type: String,
    price: i64,
    date: Option<String>,
    dest: Option<String>,
}

pub async fn run(raw: &[String], plan_id: String) -> Result<(), String> {
    let args = parse_args(raw)?;

    let conn = crate::db::connect_write().await.map_err(|e| format!("failed to connect to Turso (write tier): {e}"))?;

    // Resolve dest + validate it belongs to this plan (fail loud).
    let dest = match resolve_active_destination(&conn, &plan_id, args.dest.as_deref()).await {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    };

    // Verify domestic_accommodations has this hotel+room_type.
    let matched = find_accommodation(&conn, &dest, &args.hotel, &args.room_type).await?;
    if matched.is_none() {
        let msg = format!(
            "No accommodation found for hotel '{}' room_type '{}' in destination '{dest}' — run `travel query-accommodation --dest {dest} --date {} --hotel \"{}\"` to list available options",
            args.hotel,
            args.room_type,
            args.date.as_deref().unwrap_or("YYYY-MM-DD"),
            args.hotel,
        );
        return Err(msg);
    }

    // Ensure bookings_current category CHECK allows 'accommodation' (live DB may still have
    // the 3-value CHECK). Best-effort idempotent widen: rebuild if needed.
    ensure_accommodation_category(&conn).await;

    let now_iso = now_rfc3339();
    let now_db = now_db_datetime();
    let version_before = read_version(&conn, &plan_id).await?;
    let version_after = version_before + 1;

    // 4. UPSERT bookings_current (accommodation).
    let booking_key = format!("{plan_id}:{dest}:{}:{}", args.hotel, args.room_type)
        .replace(' ', "_");
    let title = format!("{} {}", args.hotel, args.room_type);
    let row = travel_db::repo::bookings::BookingCurrentWrite {
        booking_key: booking_key.clone(),
        trip_id: plan_id.clone(),
        destination: dest.clone(),
        category: "accommodation".to_string(),
        subtype: None,
        title: title.clone(),
        status: "booked".to_string(),
        reference: None,
        book_by: None,
        booked_at: Some(now_iso.clone()),
        source_id: None,
        offer_id: None,
        selected_date: args.date.clone(),
        price_amount: Some(args.price),
        price_currency: "TWD".to_string(),
        origin_path: "set-accommodation".to_string(),
        payload_kv: vec![
            ("hotel".to_string(), args.hotel.clone()),
            ("room_type".to_string(), args.room_type.clone()),
            ("price_twd".to_string(), args.price.to_string()),
        ],
    };
    travel_db::repo::bookings::upsert_current(&conn, &row).await?;

    // Also keep legacy `bookings` row for backward-compat viewers (some dashboards read it).
    // PK is (destination, offer_id) — synthesize offer_id from booking_key.
    let legacy_offer_id = format!("domestic:{}", booking_key);
    let _ = conn
        .execute(
            "INSERT INTO bookings (destination, offer_id, selected_date, price_per_person, price_total, currency, status, hotel_name, selected_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?4, 'TWD', 'booked', ?5, ?6, ?6) \
             ON CONFLICT(destination, offer_id) DO UPDATE SET selected_date=excluded.selected_date, price_per_person=excluded.price_per_person, price_total=excluded.price_total, status='booked', hotel_name=excluded.hotel_name, selected_at=excluded.selected_at, updated_at=excluded.updated_at",
            libsql::params![
                dest.clone(),
                legacy_offer_id.clone(),
                args.date.clone().unwrap_or_else(|| now_db.clone()),
                args.price,
                args.hotel.clone(),
                now_db.clone(),
            ],
        )
        .await
        .map_err(|e| format!("bookings legacy upsert failed: {e}"))?;

    // 5. Advance P4 to booked via shortest-legal hops.
    let current = read_process_status(&conn, &plan_id, &dest, "process_4_accommodation").await?;
    let target = "booked";
    let hops = match current.as_deref() {
        None => {
            // No row yet — create directly as booked (no prior to validate).
            travel_db::repo::process_statuses::upsert(&conn, &plan_id, &dest, "process_4_accommodation", target, &now_db).await?;
            emit_status_changed(&conn, &plan_id, &dest, "process_4_accommodation", None, target, &now_iso).await?;
            vec![]
        }
        Some(cur) if cur == target => {
            // Already booked — still ensure booking row above, but no status hops.
            vec![]
        }
        Some(cur) => {
            let hops = legal_status_path(&cur, target)?;
            for hop in &hops {
                validate_transition(Some(hop.from), hop.to, &dest, "process_4_accommodation")?;
                travel_db::repo::process_statuses::upsert(&conn, &plan_id, &dest, "process_4_accommodation", hop.to, &now_db).await?;
                emit_status_changed(&conn, &plan_id, &dest, "process_4_accommodation", Some(hop.from), hop.to, &now_iso).await?;
            }
            hops
        }
    };
    let _ = hops;

    record_operation(
        &conn,
        &plan_id,
        "set-accommodation",
        &format!("{dest} {} {} TWD {}", args.hotel, args.room_type, args.price),
        version_before,
        version_after,
        &now_db,
    )
    .await?;

    println!("✅ Booked accommodation: {} {} TWD {} for {dest} plan {plan_id}", args.hotel, args.room_type, args.price);
    Ok(())
}

fn parse_args(raw: &[String]) -> Result<Args, String> {
    let mut hotel: Option<String> = None;
    let mut room_type: Option<String> = None;
    let mut price: Option<i64> = None;
    let mut date: Option<String> = None;
    let mut dest: Option<String> = None;

    let mut i = 0;
    while i < raw.len() {
        let k = raw[i].as_str();
        match k {
            "--hotel" => {
                let v = val(raw, i, k)?;
                if v.trim().is_empty() {
                    return Err("--hotel cannot be empty".to_string());
                }
                hotel = Some(v);
                i += 2;
            }
            "--room-type" | "--room_type" => {
                let v = val(raw, i, k)?;
                if v.trim().is_empty() {
                    return Err("--room-type cannot be empty".to_string());
                }
                room_type = Some(v);
                i += 2;
            }
            "--price" => {
                let v = val(raw, i, k)?;
                let n: i64 = v.parse().map_err(|_| "--price must be an integer (TWD)".to_string())?;
                if n <= 0 {
                    return Err("--price must be > 0".to_string());
                }
                price = Some(n);
                i += 2;
            }
            "--date" => {
                let v = val(raw, i, k)?;
                if !is_iso_date(&v) {
                    return Err(format!("Invalid --date format: {v} (expected YYYY-MM-DD)"));
                }
                date = Some(v);
                i += 2;
            }
            "--dest" | "--destination" => {
                let v = val(raw, i, k)?;
                dest = Some(v);
                i += 2;
            }
            _ if crate::plan_resolver::is_resolver_flag(k) => {
                if raw.get(i + 1).is_some_and(|v| !v.starts_with("--")) {
                    i += 2;
                } else {
                    i += 1;
                }
            }
            "--help" | "-h" => {
                print_usage();
                std::process::exit(0);
            }
            other if other.starts_with("--") => {
                return Err(format!("unknown flag for set-accommodation: {other}"));
            }
            other => return Err(format!("unexpected positional argument: {other}")),
        }
    }

    let hotel = hotel.ok_or_else(|| "--hotel <name> is required".to_string())?;
    let room_type = room_type.ok_or_else(|| "--room-type <type> is required".to_string())?;
    let price = price.ok_or_else(|| "--price <twd> is required".to_string())?;

    Ok(Args { hotel, room_type, price, date, dest })
}

fn val(raw: &[String], i: usize, flag: &str) -> Result<String, String> {
    raw.get(i + 1).cloned().ok_or_else(|| format!("{flag} requires a value"))
}

fn is_iso_date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10 && b[4] == b'-' && b[7] == b'-' && b[..4].iter().all(u8::is_ascii_digit) && b[5..7].iter().all(u8::is_ascii_digit) && b[8..10].iter().all(u8::is_ascii_digit)
}

fn print_usage() {
    println!("Usage:\n  travel set-accommodation --hotel <name> --room-type <type> --price <twd> [--date YYYY-MM-DD] [--dest <slug>] [--plan-id <id>]");
}

async fn find_accommodation(
    conn: &libsql::Connection,
    dest: &str,
    hotel: &str,
    room_type: &str,
) -> Result<Option<String>, String> {
    let mut rows = conn
        .query(
            "SELECT id FROM domestic_accommodations WHERE destination = ?1 AND hotel_name = ?2 AND room_type = ?3 LIMIT 1",
            libsql::params![dest.to_string(), hotel.to_string(), room_type.to_string()],
        )
        .await
        .map_err(|e| format!("domestic_accommodations lookup failed: {e}"))?;
    if let Some(row) = rows.next().await.map_err(|e| format!("domestic_accommodations row read failed: {e}"))? {
        let id: String = row.get(0).unwrap_or_default();
        return Ok(Some(id));
    }
    Ok(None)
}

async fn read_process_status(
    conn: &libsql::Connection,
    plan_id: &str,
    dest: &str,
    process_id: &str,
) -> Result<Option<String>, String> {
    let mut rows = conn
        .query(
            "SELECT status FROM process_statuses WHERE plan_id = ?1 AND destination = ?2 AND process_id = ?3",
            libsql::params![plan_id.to_string(), dest.to_string(), process_id.to_string()],
        )
        .await
        .map_err(|e| e.to_string())?;
    if let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
        let s: String = row.get(0).unwrap_or_default();
        return Ok(if s.is_empty() { None } else { Some(s) });
    }
    Ok(None)
}

fn status_literal(s: &str) -> Option<&'static str> {
    match s {
        "pending" => Some("pending"),
        "researching" => Some("researching"),
        "researched" => Some("researched"),
        "selecting" => Some("selecting"),
        "selected" => Some("selected"),
        "populated" => Some("populated"),
        "booking" => Some("booking"),
        "booked" => Some("booked"),
        "confirmed" => Some("confirmed"),
        "skipped" => Some("skipped"),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StatusHop {
    from: &'static str,
    to: &'static str,
}

fn legal_status_path(current: &str, target: &str) -> Result<Vec<StatusHop>, String> {
    if current == target {
        return Ok(Vec::new());
    }
    let start = status_literal(current).ok_or_else(|| format!("unknown current status: {current}"))?;
    let end = status_literal(target).ok_or_else(|| format!("unknown target status: {target}"))?;

    let mut queue: VecDeque<(&'static str, Vec<StatusHop>)> = VecDeque::new();
    let mut visited: HashSet<&'static str> = HashSet::new();
    queue.push_back((start, Vec::new()));
    visited.insert(start);

    while let Some((node, path)) = queue.pop_front() {
        for &next in crate::cascade::common::allowed_transition_targets(node) {
            if visited.contains(next) {
                continue;
            }
            let mut new_path = path.clone();
            new_path.push(StatusHop { from: node, to: next });
            if next == end {
                return Ok(new_path);
            }
            visited.insert(next);
            queue.push_back((next, new_path));
        }
    }
    Err(format!("no legal status path from {current} to {target}"))
}

/// Ensure `bookings_current.category` CHECK allows 'accommodation'.
/// Idempotent: if the DDL already contains it, no-op. Otherwise rebuild the
/// table with the widened CHECK (same pattern as other CHECK widenings in
/// `db_migrate`).
async fn ensure_accommodation_category(conn: &libsql::Connection) {
    // Cheap DDL check via sqlite_master.
    let sql = "SELECT sql FROM sqlite_master WHERE type='table' AND name='bookings_current'";
    let mut rows = match conn.query(sql, ()).await {
        Ok(r) => r,
        Err(_) => return,
    };
    let ddl: Option<String> = match rows.next().await {
        Ok(Some(row)) => row.get::<String>(0).ok(),
        _ => None,
    };
    let Some(ddl) = ddl else { return };
    if ddl.contains("accommodation") {
        return;
    }
    // Rebuild with widened CHECK. Use the same columns as the canonical
    // CREATE (see db_migrate.rs step 7) but add accommodation to the CHECK.
    let rebuild = [
        "CREATE TABLE bookings_current_new (
  booking_key TEXT PRIMARY KEY,
  trip_id TEXT NOT NULL,
  destination TEXT NOT NULL,
  category TEXT NOT NULL CHECK(category IN ('package','transfer','activity','accommodation')),
  subtype TEXT,
  title TEXT NOT NULL,
  status TEXT NOT NULL CHECK(status IN ('pending','planned','booked','confirmed','waitlist','skipped','cancelled')),
  reference TEXT,
  book_by TEXT,
  booked_at TEXT,
  source_id TEXT,
  offer_id TEXT,
  selected_date TEXT,
  price_amount INTEGER,
  price_currency TEXT DEFAULT 'TWD',
  origin_path TEXT,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
);",
        "INSERT INTO bookings_current_new SELECT booking_key, trip_id, destination, category, subtype, title, status, reference, book_by, booked_at, source_id, offer_id, selected_date, price_amount, price_currency, origin_path, updated_at FROM bookings_current;",
        "DROP TABLE bookings_current;",
        "ALTER TABLE bookings_current_new RENAME TO bookings_current;",
        "CREATE INDEX IF NOT EXISTS idx_bc_dest ON bookings_current(destination, category);",
        "CREATE INDEX IF NOT EXISTS idx_bc_status ON bookings_current(status);",
        "CREATE INDEX IF NOT EXISTS idx_bc_offer ON bookings_current(offer_id);",
    ];
    for sql in rebuild {
        let _ = conn.execute(sql, ()).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_required_fields() {
        let o = parse_args(&a(&["--hotel", "海論", "--room-type", "海景雙人房", "--price", "5200"])).unwrap();
        assert_eq!(o.hotel, "海論");
        assert_eq!(o.room_type, "海景雙人房");
        assert_eq!(o.price, 5200);
    }

    #[test]
    fn parses_optional_date_and_dest() {
        let o = parse_args(&a(&["--hotel", "海論", "--room-type", "海景雙人房", "--price", "5200", "--date", "2026-09-03", "--dest", "jiufen"])).unwrap();
        assert_eq!(o.date.as_deref(), Some("2026-09-03"));
        assert_eq!(o.dest.as_deref(), Some("jiufen"));
    }

    #[test]
    fn rejects_missing_hotel() {
        let e = parse_args(&a(&["--room-type", "海景雙人房", "--price", "5200"])).unwrap_err();
        assert!(e.contains("--hotel"));
    }

    #[test]
    fn rejects_missing_room_type() {
        let e = parse_args(&a(&["--hotel", "海論", "--price", "5200"])).unwrap_err();
        assert!(e.contains("--room-type"));
    }

    #[test]
    fn rejects_missing_price() {
        let e = parse_args(&a(&["--hotel", "海論", "--room-type", "海景雙人房"])).unwrap_err();
        assert!(e.contains("--price"));
    }

    #[test]
    fn rejects_non_integer_price() {
        let e = parse_args(&a(&["--hotel", "海論", "--room-type", "海景雙人房", "--price", "5k"])).unwrap_err();
        assert!(e.contains("--price"));
    }

    #[test]
    fn rejects_bad_date() {
        let e = parse_args(&a(&["--hotel", "海論", "--room-type", "海景雙人房", "--price", "5200", "--date", "2026/09/03"])).unwrap_err();
        assert!(e.contains("Invalid --date"));
    }

    #[test]
    fn rejects_unknown_flag() {
        let e = parse_args(&a(&["--hotel", "海論", "--room-type", "海景雙人房", "--price", "5200", "--bogus"])).unwrap_err();
        assert!(e.contains("unknown flag"));
    }
}
