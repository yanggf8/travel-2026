// `travel chat-format --run <run_id> [--region okinawa] [--max N] [--hotel "substring"]`
//
// Port of src/cli/commands/chat-format.ts. Reads shaping_tour_group_offers
// (+ shaping_tour_group_offer_notes) for a run, filters to "qualified"
// candidates (return on/before 2026-06-27, price > 10000, has hotel), and
// prints a clean messenger/chat block per candidate (ready to paste).
//
// Read-only, plain text. Mirrors the TS service listTourGroupOffers WHERE
// clauses (run_id + dest_region) and the TS output formatting exactly.

use crate::db;

struct OfferRow {
    offer_id: String,
    source_id: String,
    depart_date: String,
    return_date: String,
    nights: i64,
    price_per_person_twd: i64,
    title: String,
    url: String,
    hotel_name: Option<String>,
    seats_available: Option<i64>,
    raw_note: Option<String>,
    raw_flight: Option<String>,
    raw_flight_outbound: Option<String>,
    raw_flight_return: Option<String>,
    notes: Vec<(String, String)>, // (key, value)
}

pub async fn run(args: &[String]) -> Result<(), String> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!(
            "Usage:\n  travel chat-format --run <run_id> [--region okinawa] [--max N] [--hotel \"substring\"]"
        );
        return Ok(());
    }

    let mut run_id: Option<String> = None;
    let mut region = "okinawa".to_string();
    let mut max: usize = 20;
    let mut hotel_filter: Option<String> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--run" => {
                i += 1;
                run_id = args.get(i).cloned();
            }
            "--region" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    region = v.clone();
                }
            }
            "--max" => {
                i += 1;
                max = args.get(i).and_then(|s| s.parse::<usize>().ok()).unwrap_or(20);
            }
            "--hotel" => {
                i += 1;
                hotel_filter = args.get(i).cloned();
            }
            _ => {}
        }
        i += 1;
    }

    let Some(run_id) = run_id else {
        eprintln!("Error: --run <run_id> is required");
        std::process::exit(1);
    };

    let conn = db::connect_read().await?;

    // Mirror listTourGroupOffers: filter by run_id + dest_region, ORDER BY price ASC.
    let mut rows = conn
        .query(
            "SELECT offer_id, source_id, depart_date, return_date, nights, \
             price_per_person_twd, title, url, hotel_name, seats_available, \
             raw_note, raw_flight, raw_flight_outbound, raw_flight_return \
             FROM shaping_tour_group_offers \
             WHERE run_id = ?1 AND dest_region = ?2 \
             ORDER BY price_per_person_twd ASC",
            libsql::params![run_id.clone(), region.clone()],
        )
        .await
        .map_err(|e| format!("query shaping_tour_group_offers failed: {e}"))?;

    let mut offers: Vec<OfferRow> = Vec::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("row fetch failed: {e}"))?
    {
        offers.push(OfferRow {
            offer_id: row.get::<String>(0).unwrap_or_default(),
            source_id: row.get::<String>(1).unwrap_or_default(),
            depart_date: row.get::<String>(2).unwrap_or_default(),
            return_date: row.get::<String>(3).unwrap_or_default(),
            nights: row.get::<i64>(4).unwrap_or_default(),
            price_per_person_twd: row.get::<i64>(5).unwrap_or_default(),
            title: row.get::<String>(6).unwrap_or_default(),
            url: row.get::<String>(7).unwrap_or_default(),
            hotel_name: row.get::<Option<String>>(8).unwrap_or(None),
            seats_available: row.get::<Option<i64>>(9).unwrap_or(None),
            raw_note: row.get::<Option<String>>(10).unwrap_or(None),
            raw_flight: row.get::<Option<String>>(11).unwrap_or(None),
            raw_flight_outbound: row.get::<Option<String>>(12).unwrap_or(None),
            raw_flight_return: row.get::<Option<String>>(13).unwrap_or(None),
            notes: Vec::new(),
        });
    }

    // Attach notes child rows (one per offer) — same as the TS service.
    for o in offers.iter_mut() {
        let mut nr = conn
            .query(
                "SELECT key, value FROM shaping_tour_group_offer_notes \
                 WHERE run_id = ?1 AND offer_id = ?2 ORDER BY sort_order",
                libsql::params![run_id.clone(), o.offer_id.clone()],
            )
            .await
            .map_err(|e| format!("query notes failed: {e}"))?;
        while let Some(row) = nr
            .next()
            .await
            .map_err(|e| format!("notes row fetch failed: {e}"))?
        {
            let key: String = row.get(0).unwrap_or_default();
            let value: String = row.get(1).unwrap_or_default();
            o.notes.push((key, value));
        }
    }

    // Qualified: return <= 2026-06-27, price > 10000, has hotel.
    let mut qualified: Vec<&OfferRow> = offers
        .iter()
        .filter(|r| {
            r.return_date.as_str() <= "2026-06-27"
                && r.price_per_person_twd > 10000
                && r.hotel_name.as_deref().map(|h| !h.is_empty()).unwrap_or(false)
        })
        .collect();

    if let Some(ref needle) = hotel_filter {
        let needle = needle.to_lowercase();
        qualified.retain(|r| {
            r.hotel_name
                .as_deref()
                .unwrap_or("")
                .to_lowercase()
                .contains(&needle)
        });
    }

    // Sort by depart_date, then price.
    qualified.sort_by(|a, b| {
        a.depart_date
            .cmp(&b.depart_date)
            .then(a.price_per_person_twd.cmp(&b.price_per_person_twd))
    });
    qualified.truncate(max);

    if qualified.is_empty() {
        println!("No qualified candidates found.");
        return Ok(());
    }

    for r in qualified {
        let agent = r.source_id.to_uppercase();
        let hotel = r.hotel_name.as_deref().unwrap_or("");
        // Parity with TS: price_per_person_twd arrives as a STRING from the
        // Turso pipeline, so JS `.toLocaleString()` on it is a no-op (no
        // grouping). The total is `price * 2`, a Number, so it IS grouped.
        let price = r.price_per_person_twd.to_string();
        let total = fmt_thousands(r.price_per_person_twd * 2);

        // Flight line — from the de-JSON'd typed columns.
        let mut flight_line = "待確認".to_string();
        if let Some(f) = &r.raw_flight {
            if !f.is_empty() {
                flight_line = f.clone();
            }
        }
        if let (Some(out), Some(ret)) = (&r.raw_flight_outbound, &r.raw_flight_return) {
            if !out.is_empty() && !ret.is_empty() {
                flight_line = format!("{out} / {ret}");
            }
        }

        let baggage = "7kg + 20kg"; // default for most FIT we see

        // 美栄橋 mention check — scan the typed note + every annotation value.
        let mut annotation_text = r.raw_note.clone().unwrap_or_default();
        for (_, v) in &r.notes {
            annotation_text.push(' ');
            annotation_text.push_str(v);
        }
        let location = if annotation_text.contains("美栄橋") || hotel.contains("水之都") {
            "美栄橋站附近，國際通走路方便"
        } else {
            "中央那霸"
        };

        let seats = match r.seats_available {
            Some(s) => s.to_string(),
            None => "待確認".to_string(),
        };

        // Rating — a notes annotation (key 'rating').
        let rating = r
            .notes
            .iter()
            .find(|(k, _)| k == "rating")
            .map(|(_, v)| v.clone())
            .unwrap_or_default();

        let note = if let Some(n) = &r.raw_note {
            if !n.is_empty() { n.clone() } else { r.title.clone() }
        } else {
            r.title.clone()
        };

        println!("{agent} | {hotel}");
        println!("日期: {} → {} ({}晚)", r.depart_date, r.return_date, r.nights);
        println!("價格: ${price} /人 (2人 ${total})");
        println!("航班: {flight_line}");
        println!("行李: {baggage}");
        println!("位置: {location}");
        println!("座位: {seats}");
        if !rating.is_empty() {
            println!("評價: {rating}");
        }
        println!("重點: {note}");
        println!("連結: {}", r.url);
        println!(); // blank line between entries
    }

    Ok(())
}

/// Format an integer with thousands separators (matches JS toLocaleString for
/// en-US grouping, which is what the TS path used).
fn fmt_thousands(n: i64) -> String {
    let neg = n < 0;
    let digits = n.abs().to_string();
    let bytes = digits.as_bytes();
    let mut out = String::new();
    let len = bytes.len();
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (len - i) % 3 == 0 {
            out.push(',');
        }
        out.push(*b as char);
    }
    if neg {
        format!("-{out}")
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thousands() {
        assert_eq!(fmt_thousands(0), "0");
        assert_eq!(fmt_thousands(999), "999");
        assert_eq!(fmt_thousands(1000), "1,000");
        assert_eq!(fmt_thousands(31888), "31,888");
        assert_eq!(fmt_thousands(63776), "63,776");
    }
}
