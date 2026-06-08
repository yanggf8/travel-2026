mod bookings;
mod compare;
mod compare_dates;
mod compare_true_cost;
mod db;
mod db_status;
mod destination_ref;
mod flights;
mod freshness;
mod leave;
mod offers;
mod plans;

use std::{env, io::Read, process};

#[tokio::main]
async fn main() {
    if let Err(err) = run(env::args().skip(1).collect()).await {
        eprintln!("{err}");
        process::exit(1);
    }
}

async fn run(args: Vec<String>) -> Result<(), String> {
    match args.as_slice() {
        [] => {
            print_usage();
            Ok(())
        }
        [cmd] if cmd == "--help" || cmd == "-h" => {
            print_usage();
            Ok(())
        }
        [group, sub, rest @ ..] if group == "leave" && sub == "calc" => leave_calc(rest).await,
        [group, sub, rest @ ..] if group == "compare" && sub == "trips" => {
            compare_trips(rest).await
        }
        [group, sub, rest @ ..] if group == "compare" && sub == "dates" => {
            if rest.iter().any(|a| a == "--help" || a == "-h") {
                println!("Usage:\n  travel compare dates --start YYYY-MM-DD --end YYYY-MM-DD [--nights N] [--hotel-per-night TWD] [--market taiwan] [--region r] [--destination d] [--pax N] [--baggage-fee TWD]");
                return Ok(());
            }
            let opts = compare_dates::CompareDatesArgs::parse(rest)?;
            compare_dates::run(&opts).await
        }
        [group, sub, rest @ ..] if group == "compare" && (sub == "true-cost" || sub == "truecost") => {
            if rest.iter().any(|a| a == "--help" || a == "-h") {
                println!("Usage:\n  travel compare true-cost --region <name> [--date YYYY-MM-DD] [--pax N] [--itinerary \"kyoto:1,osaka:2\"] [--jpy-rate N]");
                return Ok(());
            }
            let opts = compare_true_cost::TrueCostArgs::parse(rest)?;
            compare_true_cost::run(&opts).await
        }
        [group, sub, rest @ ..] if group == "normalize" && sub == "flights" => {
            normalize_flights(rest)
        }
        [cmd] if cmd == "plans" => plans::run().await,
        [cmd, rest @ ..] if cmd == "query-offers" => {
            let opts = offers::OffersArgs::parse(rest)?;
            offers::run(&opts).await
        }
        [cmd, rest @ ..] if cmd == "query-destination-ref" || cmd == "destination-ref" => {
            let opts = destination_ref::DestRefArgs::parse(rest)?;
            destination_ref::run(&opts).await
        }
        [cmd, rest @ ..] if cmd == "query-bookings" => {
            let opts = bookings::QueryBookingsArgs::parse(rest)?;
            bookings::run(&opts).await
        }
        [cmd, rest @ ..] if cmd == "check-freshness" => {
            let opts = freshness::FreshnessArgs::parse(rest)?;
            freshness::run(&opts).await
        }
        [group, sub, rest @ ..] if group == "db" && sub == "status" => {
            if !rest.is_empty() {
                return Err(
                    "Usage: travel db status\n  (no arguments; reads Turso via turso-util)"
                        .to_string(),
                );
            }
            db_status::run().await
        }
        _ => Err(format!(
            "unknown command: {}\nRun `travel --help` for usage.",
            args.join(" ")
        )),
    }
}

async fn leave_calc(args: &[String]) -> Result<(), String> {
    if args.len() < 2 || args.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!(
            "Usage:\n  travel leave calc <start-date> <end-date> [country]\n\nExample:\n  travel leave calc 2026-06-20 2026-06-24 taiwan"
        );
        return Ok(());
    }
    let country = args.get(2).map(String::as_str).unwrap_or("taiwan");
    let year = leave::year_from_date(&args[0])?;
    let calendar = db::load_holiday_calendar(country, year).await?;
    let result = leave::calculate_leave_days(&args[0], &args[1], &calendar)?;
    println!("{}", leave::format_leave_day_table(&result));
    Ok(())
}

async fn compare_trips(args: &[String]) -> Result<(), String> {
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        compare::print_usage();
        return Ok(());
    }
    let opts = compare::CompareArgs::parse(args)?;
    let year = opts.year()?;
    let calendar = db::load_holiday_calendar(&opts.market, year).await?;
    let output = compare::compare_trips(&opts, &calendar)?;
    println!("{output}");
    Ok(())
}

fn normalize_flights(args: &[String]) -> Result<(), String> {
    if args.iter().any(|arg| arg == "--help" || arg == "-h") || args.is_empty() {
        flights::print_usage();
        return Ok(());
    }
    let opts = flights::NormalizeArgs::parse(args)?;
    let raw_text = if opts.stdin {
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .map_err(|err| format!("failed to read stdin: {err}"))?;
        buf
    } else {
        opts.text.clone().ok_or_else(|| {
            "missing flight text; pass --text '<rendered text>' or --stdin".to_string()
        })?
    };
    let result =
        flights::normalize_text(&opts.label, opts.url.as_deref().unwrap_or(""), &raw_text)?;
    println!("{}", flights::format_search_result(&result));
    Ok(())
}

fn print_usage() {
    println!(
        "Travel CLI\n\nUsage:\n  travel plans\n  travel query-offers [--source a,b] [--region r] [--dest d] [--max-price N] [--start YYYY-MM-DD] [--end YYYY-MM-DD] [--limit N]\n  travel query-destination-ref --slug <destination_slug>\n  travel query-bookings [--trip-id id] [--dest slug] [--category c] [--status s] [--max N]\n  travel check-freshness --source <id> [--region r] [--start YYYY-MM-DD] [--end YYYY-MM-DD] [--max-age N] [--plan-id id] [--dest slug]\n  travel compare trips --trip '<key=value;...>' [--trip '<key=value;...>'] [--market taiwan] [--detailed]\n  travel normalize flights --text '<rendered flight text>' --url '<source url>' [--label name]\n  travel normalize flights --stdin --url '<source url>' [--label name]\n  travel leave calc <start-date> <end-date> [country]\n\nRules:\n  plain-text input and output; no JSON files or JSON output"
    );
}
