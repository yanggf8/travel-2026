// `travel query-offers` — read offers from the Turso `offers` table (raw,
// non-plan path). Read-only, plain-text table. Ports the raw-offers query +
// printTursoOfferTable format from src/services/turso-service.ts.
// Note: the `raw_data` column was dropped in the no-JSON refactor — not selected.

use crate::db;

pub struct OffersArgs {
    pub source: Option<String>,
    pub region: Option<String>,
    pub destination: Option<String>,
    pub max_price: Option<i64>,
    pub start: Option<String>,
    pub end: Option<String>,
    pub limit: i64,
}

impl OffersArgs {
    pub fn parse(args: &[String]) -> Result<Self, String> {
        let mut o = OffersArgs {
            source: None,
            region: None,
            destination: None,
            max_price: None,
            start: None,
            end: None,
            limit: 500,
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
                "--source" => o.source = Some(val()?),
                "--region" => o.region = Some(val()?),
                "--dest" | "--destination" => o.destination = Some(val()?),
                "--max-price" => {
                    o.max_price = Some(val()?.parse().map_err(|_| "--max-price must be an integer".to_string())?)
                }
                "--start" => o.start = Some(val()?),
                "--end" => o.end = Some(val()?),
                "--limit" => {
                    o.limit = val()?.parse().map_err(|_| "--limit must be an integer".to_string())?
                }
                other => return Err(format!("unknown flag for query-offers: {other}")),
            }
            i += 2;
        }
        Ok(o)
    }
}

pub async fn run(opts: &OffersArgs) -> Result<(), String> {
    // Dynamic WHERE + bound params via the DAL (no sql_quote / string interpolation).
    let mut filter = travel_db::repo::offers::OfferFilter::new();
    if let Some(d) = &opts.destination {
        filter = filter.destination(d);
    }
    if let Some(r) = &opts.region {
        filter = filter.region(r);
    }
    if let Some(s) = &opts.source {
        // comma-separated source list → IN (...)
        filter = filter.source_id_in_csv(s);
    }
    if let Some(mp) = opts.max_price {
        filter = filter.max_price(mp);
    }
    if let Some(s) = &opts.start {
        filter = filter.departure_from(s);
    }
    if let Some(e) = &opts.end {
        filter = filter.departure_to(e);
    }
    let where_built = filter.build();

    let sql = format!(
        "SELECT source_id, type, price_per_person, hotel_name, airline, departure_date, scraped_at \
         FROM offers {} ORDER BY scraped_at DESC, price_per_person ASC LIMIT {}",
        where_built.clause, opts.limit
    );

    let conn = db::connect_read().await?;
    let mut rows = conn
        .query(sql.as_str(), where_built.params)
        .await
        .map_err(|err| format!("failed to query offers from Turso: {err}"))?;

    let mut out: Vec<[String; 7]> = Vec::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|err| format!("failed to read offer row: {err}"))?
    {
        let source: String = row.get(0).unwrap_or_default();
        let kind: String = row.get(1).unwrap_or_default();
        let price: Option<i64> = row.get(2).ok();
        let hotel: String = row.get(3).unwrap_or_default();
        let airline: String = row.get(4).unwrap_or_default();
        let depart: String = row.get(5).unwrap_or_default();
        let scraped: String = row.get(6).unwrap_or_default();
        out.push([
            source,
            kind,
            price.map(|p| p.to_string()).unwrap_or_else(|| "-".to_string()),
            hotel,
            airline,
            depart,
            scraped,
        ]);
    }

    print_offer_table(&out);
    Ok(())
}

fn dash(s: &str) -> String {
    if s.is_empty() { "-".to_string() } else { s.to_string() }
}

fn print_offer_table(rows: &[[String; 7]]) {
    if rows.is_empty() {
        println!("\nNo offers found in Turso.");
        return;
    }
    println!("\nTurso Offers ({} results):", rows.len());
    let bar = "─".repeat(100);
    println!("{bar}");
    let header = [
        format!("{:<12}", "Source"),
        format!("{:<8}", "Type"),
        format!("{:>8}", "Price"),
        format!("{:<25}", "Hotel"),
        format!("{:<10}", "Airline"),
        format!("{:<12}", "Depart"),
        format!("{:<20}", "Scraped"),
    ]
    .join(" │ ");
    println!("{header}");
    println!("{bar}");
    for r in rows {
        let hotel = {
            let h = dash(&r[3]);
            let truncated: String = h.chars().take(25).collect();
            format!("{truncated:<25}")
        };
        let scraped = {
            let s: String = r[6].chars().take(19).collect();
            let s = if s.is_empty() { "-".to_string() } else { s };
            format!("{s:<20}")
        };
        let line = [
            format!("{:<12}", dash(&r[0])),
            format!("{:<8}", dash(&r[1])),
            format!("{:>8}", r[2]),
            hotel,
            format!("{:<10}", dash(&r[4])),
            format!("{:<12}", dash(&r[5])),
            scraped,
        ]
        .join(" │ ");
        println!("{line}");
    }
}
