// `travel query-tour-group-offers --run <run_id> [--source <id>]
//   [--dest-region <region>] [--nights N] [--max-price TWD]`
//
// Port of the query-tour-group-offers command in src/cli/commands/tour-group.ts
// (service: listTourGroupOffers in src/services/tour-group-service.ts).
// Reads shaping_tour_group_offers, ordered by price_per_person_twd ASC.
// Read-only, plain text only (agent-first; no user-facing JSON).

use crate::db;

struct OfferRow {
    source_id: String,
    dest_region: String,
    depart_date: String,
    // Selected from the DB and available, but not shown in the plain-text
    // table (they were only emitted by the removed --json output).
    #[allow(dead_code)]
    return_date: String,
    nights: i64,
    price_per_person_twd: i64,
    title: String,
    hotel_name: Option<String>,
    hotel_star_rating: Option<i64>,
    #[allow(dead_code)]
    product_kind: Option<String>,
}

pub async fn run(rest: &[String]) -> Result<(), String> {
    if rest.iter().any(|a| a == "--help" || a == "-h") {
        println!(
            "Usage:\n  travel query-tour-group-offers --run <run_id> [--source <id>] \
             [--dest-region <region>] [--nights N] [--max-price TWD]"
        );
        return Ok(());
    }

    let mut run_id: Option<String> = None;
    let mut source: Option<String> = None;
    let mut dest_region: Option<String> = None;
    let mut nights: Option<i64> = None;
    let mut max_price: Option<i64> = None;

    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "--run" => { i += 1; run_id = rest.get(i).cloned(); }
            "--source" => { i += 1; source = rest.get(i).cloned(); }
            "--dest-region" => { i += 1; dest_region = rest.get(i).cloned(); }
            "--nights" => {
                i += 1;
                nights = rest.get(i).and_then(|s| s.parse::<i64>().ok());
            }
            "--max-price" => {
                i += 1;
                max_price = rest.get(i).and_then(|s| s.parse::<i64>().ok());
            }
            _ => {}
        }
        i += 1;
    }

    let Some(run_id) = run_id else {
        eprintln!("Error: query-tour-group-offers requires --run <run_id>");
        std::process::exit(1);
    };

    let conn = db::connect_read().await?;

    // Build the filtered SELECT (mirrors listTourGroupOffers WHERE clauses).
    let mut sql = String::from(
        "SELECT source_id, dest_region, depart_date, return_date, nights, \
         price_per_person_twd, title, hotel_name, hotel_star_rating, product_kind \
         FROM shaping_tour_group_offers WHERE run_id = ?1",
    );
    let mut binds: Vec<libsql::Value> = vec![run_id.clone().into()];
    let mut n = 2;
    if let Some(ref s) = source {
        sql.push_str(&format!(" AND source_id = ?{n}"));
        binds.push(s.clone().into());
        n += 1;
    }
    if let Some(ref r) = dest_region {
        sql.push_str(&format!(" AND dest_region = ?{n}"));
        binds.push(r.clone().into());
        n += 1;
    }
    if let Some(v) = nights {
        sql.push_str(&format!(" AND nights = ?{n}"));
        binds.push(v.into());
        n += 1;
    }
    if let Some(v) = max_price {
        sql.push_str(&format!(" AND price_per_person_twd <= ?{n}"));
        binds.push(v.into());
    }
    sql.push_str(" ORDER BY price_per_person_twd ASC");

    let mut rows = conn
        .query(&sql, libsql::params_from_iter(binds))
        .await
        .map_err(|e| format!("query shaping_tour_group_offers failed: {e}"))?;

    let mut out: Vec<OfferRow> = Vec::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("row fetch failed: {e}"))?
    {
        out.push(OfferRow {
            source_id: row.get::<String>(0).unwrap_or_default(),
            dest_region: row.get::<String>(1).unwrap_or_default(),
            depart_date: row.get::<String>(2).unwrap_or_default(),
            return_date: row.get::<String>(3).unwrap_or_default(),
            nights: row.get::<i64>(4).unwrap_or_default(),
            price_per_person_twd: row.get::<i64>(5).unwrap_or_default(),
            title: row.get::<String>(6).unwrap_or_default(),
            hotel_name: row.get::<Option<String>>(7).unwrap_or(None),
            hotel_star_rating: row.get::<Option<i64>>(8).unwrap_or(None),
            product_kind: row.get::<Option<String>>(9).unwrap_or(None),
        });
    }

    if out.is_empty() {
        println!("(no rows)");
        return Ok(());
    }

    // Header + rows mirror the TS column layout.
    println!(
        "PRICE      SOURCE     REGION    DEPART      NIGHTS  HOTEL                          STAR  TITLE"
    );
    println!("{}", "-".repeat(120));
    for r in &out {
        let price = format!("{:>8}", r.price_per_person_twd);
        let src = pad_end(&r.source_id, 10);
        let reg = pad_end(&r.dest_region, 9);
        let dt = pad_end(&r.depart_date, 11);
        let nights = format!("{:>6}", r.nights);
        let hotel = pad_end(&truncate(r.hotel_name.as_deref().unwrap_or(""), 30), 30);
        let star = match r.hotel_star_rating {
            None => "   -".to_string(),
            Some(s) => format!("   {s}"),
        };
        let title = truncate(&r.title, 30);
        println!("{price}  {src} {reg} {dt} {nights}  {hotel}  {star}  {title}");
    }

    Ok(())
}

fn pad_end(s: &str, width: usize) -> String {
    let len = s.chars().count();
    if len >= width {
        s.to_string()
    } else {
        format!("{s}{}", " ".repeat(width - len))
    }
}

fn truncate(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}
