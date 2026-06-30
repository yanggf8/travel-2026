// `travel query-bookings` — read the `bookings_current` table with filters.
// Read-only, plain-text table. Ports queryBookings + printBookingsTable from
// src/services/turso-service.ts (and the query-bookings handler in
// src/cli/commands/turso.ts).

use crate::db;

pub struct QueryBookingsArgs {
    pub trip_id: Option<String>,
    pub destination: Option<String>,
    pub category: Option<String>,
    pub status: Option<String>,
    pub limit: i64,
}

impl QueryBookingsArgs {
    pub fn parse(args: &[String]) -> Result<Self, String> {
        let mut o = QueryBookingsArgs {
            trip_id: None,
            destination: None,
            category: None,
            status: None,
            // TS default is LIMIT 100 when --max absent.
            limit: 100,
        };
        let mut i = 0;
        while i < args.len() {
            let key = args[i].as_str();
            let val = || {
                args.get(i + 1)
                    .cloned()
                    .ok_or_else(|| format!("{key} requires a value"))
            };
            match key {
                "--trip-id" => o.trip_id = Some(val()?),
                "--dest" | "--destination" => o.destination = Some(val()?),
                "--category" => o.category = Some(val()?),
                "--status" => o.status = Some(val()?),
                "--max" => {
                    o.limit = val()?.parse().map_err(|_| "--max must be an integer".to_string())?
                }
                other => return Err(format!("unknown flag for query-bookings: {other}")),
            }
            i += 2;
        }
        Ok(o)
    }
}

pub async fn run(opts: &QueryBookingsArgs) -> Result<(), String> {
    use travel_db::repo::bookings::{self, BookingsCurrentFilter};

    let conn = db::connect_read().await?;
    let dal_rows = bookings::query_current(
        &conn,
        &BookingsCurrentFilter {
            trip_id: opts.trip_id.as_deref(),
            destination: opts.destination.as_deref(),
            category: opts.category.as_deref(),
            status: opts.status.as_deref(),
        },
        opts.limit,
    )
    .await?;

    // Only the columns the table renders: category, status, title, price, book_by, reference.
    let out: Vec<BookingRow> = dal_rows
        .into_iter()
        .map(|r| BookingRow {
            category: r.category,
            status: r.status,
            title: r.title,
            reference: r.reference,
            book_by: r.book_by,
            price: r.price,
        })
        .collect();

    print_bookings_table(&out);
    Ok(())
}

struct BookingRow {
    category: String,
    status: String,
    title: String,
    reference: String,
    book_by: String,
    price: Option<i64>,
}

fn dash(s: &str) -> String {
    if s.is_empty() {
        "-".to_string()
    } else {
        s.to_string()
    }
}

fn print_bookings_table(rows: &[BookingRow]) {
    if rows.is_empty() {
        println!("\nNo bookings found.");
        return;
    }

    println!("\nBookings ({} rows):", rows.len());
    let bar = "─".repeat(95);
    println!("{bar}");
    let header = [
        format!("{:<10}", "Category"),
        format!("{:<10}", "Status"),
        format!("{:<40}", "Title"),
        format!("{:>8}", "Price"),
        format!("{:<12}", "Book By"),
        format!("{:<10}", "Ref"),
    ]
    .join(" │ ");
    println!("{header}");
    println!("{bar}");

    for r in rows {
        let price = r.price.map(|p| p.to_string()).unwrap_or_else(|| "-".to_string());
        let title = {
            let t = dash(&r.title);
            let truncated: String = t.chars().take(40).collect();
            format!("{truncated:<40}")
        };
        let reference = {
            let ref_full = dash(&r.reference);
            let truncated: String = ref_full.chars().take(10).collect();
            format!("{truncated:<10}")
        };
        let line = [
            format!("{:<10}", dash(&r.category)),
            format!("{:<10}", dash(&r.status)),
            title,
            format!("{price:>8}"),
            format!("{:<12}", dash(&r.book_by)),
            reference,
        ]
        .join(" │ ");
        println!("{line}");
    }

    println!("{bar}");
}
