// `travel query-offers` — read offers from the Turso `offers` table (raw,
// non-plan path). Read-only, plain-text table. Ports the raw-offers query +
// printTursoOfferTable format from src/services/turso-service.ts.
// Note: the `raw_data` column was dropped in the no-JSON refactor — not selected.

use crate::db;

#[derive(Debug)]
pub struct OffersArgs {
    pub source: Option<String>,
    pub region: Option<String>,
    pub destination: Option<String>,
    pub max_price: Option<i64>,
    pub start: Option<String>,
    pub end: Option<String>,
    pub limit: i64,
    pub capture_id: Option<String>,
    pub job_id: Option<String>,
    pub attempt_id: Option<String>,
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
            capture_id: None,
            job_id: None,
            attempt_id: None,
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
                "--capture-id" => o.capture_id = Some(val()?),
                "--job-id" => o.job_id = Some(val()?),
                "--attempt-id" => o.attempt_id = Some(val()?),
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
    if let Some(c) = &opts.capture_id {
        filter = filter.capture_id(c);
    }
    if let Some(j) = &opts.job_id {
        filter = filter.produced_by_job_id(j);
    }
    if let Some(a) = &opts.attempt_id {
        filter = filter.produced_by_attempt_id(a);
    }
    let where_built = filter.build();

    let sql = format!(
        "SELECT source_id, type, price_per_person, hotel_name, airline, departure_date, scraped_at, \
         capture_id, produced_by_job_id, produced_by_attempt_id \
         FROM offers {} ORDER BY scraped_at DESC, price_per_person ASC LIMIT {}",
        where_built.clause, opts.limit
    );

    let conn = db::connect_read().await?;
    let mut rows = conn
        .query(sql.as_str(), where_built.params)
        .await
        .map_err(|err| format!("failed to query offers from Turso: {err}"))?;

    let mut out: Vec<[String; 10]> = Vec::new();
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
        let capture: String = row.get(7).unwrap_or_default();
        let job: String = row.get(8).unwrap_or_default();
        let attempt: String = row.get(9).unwrap_or_default();
        out.push([
            source,
            kind,
            price.map(|p| p.to_string()).unwrap_or_else(|| "-".to_string()),
            hotel,
            airline,
            depart,
            scraped,
            capture,
            job,
            attempt,
        ]);
    }

    print_offer_table(&out);
    Ok(())
}

fn dash(s: &str) -> String {
    if s.is_empty() { "-".to_string() } else { s.to_string() }
}

fn print_offer_table(rows: &[[String; 10]]) {
    if rows.is_empty() {
        println!("\nNo offers found in Turso.");
        return;
    }
    println!("\nTurso Offers ({} results):", rows.len());
    let bar = "─".repeat(140);
    println!("{bar}");
    let header = [
        format!("{:<12}", "Source"),
        format!("{:<8}", "Type"),
        format!("{:>8}", "Price"),
        format!("{:<25}", "Hotel"),
        format!("{:<10}", "Airline"),
        format!("{:<12}", "Depart"),
        format!("{:<20}", "Scraped"),
        format!("{:<16}", "Capture"),
        format!("{:<16}", "Job"),
        format!("{:<16}", "Attempt"),
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
        let col16 = |s: &str| {
            let d = dash(s);
            let truncated: String = d.chars().take(16).collect();
            format!("{truncated:<16}")
        };
        let line = [
            format!("{:<12}", dash(&r[0])),
            format!("{:<8}", dash(&r[1])),
            format!("{:>8}", r[2]),
            hotel,
            format!("{:<10}", dash(&r[4])),
            format!("{:<12}", dash(&r[5])),
            scraped,
            col16(&r[7]),
            col16(&r[8]),
            col16(&r[9]),
        ]
        .join(" │ ");
        println!("{line}");
    }
}

#[cfg(test)]
mod tests {
    use super::OffersArgs;

    #[test]
    fn parse_accepts_provenance_filters() {
        let o = OffersArgs::parse(&[
            "--capture-id".into(),
            "cap-1".into(),
            "--job-id".into(),
            "job-1".into(),
            "--attempt-id".into(),
            "att-1".into(),
            "--dest".into(),
            "tokyo_2026".into(),
        ])
        .expect("parse");
        assert_eq!(o.capture_id.as_deref(), Some("cap-1"));
        assert_eq!(o.job_id.as_deref(), Some("job-1"));
        assert_eq!(o.attempt_id.as_deref(), Some("att-1"));
        assert_eq!(o.destination.as_deref(), Some("tokyo_2026"));
    }

    #[test]
    fn parse_rejects_unknown_flag() {
        let err = OffersArgs::parse(&["--totally-bogus".into()]).unwrap_err();
        assert!(err.contains("unknown flag"), "err={err}");
    }
}
