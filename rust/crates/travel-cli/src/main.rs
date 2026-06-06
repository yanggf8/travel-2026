mod compare;
mod db;
mod flights;
mod leave;
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
        [group, sub, rest @ ..] if group == "normalize" && sub == "flights" => {
            normalize_flights(rest)
        }
        [cmd] if cmd == "plans" => plans::run().await,
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
        "Travel CLI\n\nUsage:\n  travel plans\n  travel compare trips --trip '<key=value;...>' [--trip '<key=value;...>'] [--market taiwan] [--detailed]\n  travel normalize flights --text '<rendered flight text>' --url '<source url>' [--label name]\n  travel normalize flights --stdin --url '<source url>' [--label name]\n  travel leave calc <start-date> <end-date> [country]\n\nRules:\n  plain-text input and output; no JSON files or JSON output"
    );
}
