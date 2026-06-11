mod bookings;
mod cascade;
mod compare;
mod compare_dates;
mod compare_true_cost;
mod db;
mod db_exec;
mod db_query_offers;
mod db_status;
mod destination_ref;
mod flights;
mod freshness;
mod import_offers;
mod leave;
mod offers;
mod plan;
mod plan_resolver;
mod plans;
mod scrape_parser;
mod set_activity;
mod set_activity_poi;
mod set_airport_transfer;
mod set_dates;
mod set_day_theme;
mod set_flight;
mod set_hotel;
mod set_route_segment;
mod set_tod;
mod share_token;
mod status;
mod tour_group_offers;
mod add_offer;
mod add_besttour_offer;
mod add_lifetour_offer;
mod import_tour_group_offers;
mod update_offer;
mod validate;
mod view_bookings;
mod view_itinerary;
mod view_transport;
// P1 Rust-port batches (docs/plans/2026-06-10-rust-port-audit.md).
// Modules are filled in by per-batch work; dispatch arms below are pre-wired so
// batches add their own file without touching main.rs (no merge collisions).
mod scaffold_itinerary; // batch 2
mod populate_itinerary; // batch 2
mod swap_days;          // batch 2
mod mark_booked;        // batch 3
mod sync_bookings;      // batch 3
mod booking_integrity;  // batch 3 (check-booking-integrity)
mod ops;                // batch 3 (run-status / run-list)
mod validate_itinerary; // batch 3
mod shaping;            // batch 4 (shaping-init/compare/adopt/baseline/export/import)
mod query_tour_group;   // batch 4 (query-tour-group-offers)
mod tour_group_bridge;  // adopt-time audit-set bridge (used by shaping-adopt --create-plan)
mod weather;            // batch 5 (fetch-weather)
mod view_prices;        // batch 5
mod search_compare;     // batch 5 (compare-offers / search-offers)
mod chat_format;        // batch 5
// P2 scripts/ port (docs/plans/2026-06-10-rust-port-audit.md §4). Pre-wired
// dispatch + stubs for collision-free fan-out (same method as P1).
mod db_migrate;         // db migrate (port of scripts/turso-migrate.ts inline DDL)
mod db_seed_plans;      // db seed plans (scripts/seed-plans-current.ts)
mod db_sync_destinations; // db sync destinations (scripts/turso-sync-destinations.ts)
mod db_sync_events;     // db sync events (scripts/turso-sync-events.ts)
mod db_fetch_holidays;  // db fetch holidays (scripts/fetch-taiwan-holidays.ts)

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
        [cmd] if cmd == "plans" || cmd == "list-plans" => plans::run().await,
        [cmd, rest @ ..] if cmd == "resolve-plan" => plan_resolver::run_cli(rest).await,
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
        [cmd, rest @ ..] if cmd == "import-offers" => {
            if rest.iter().any(|a| a == "--help" || a == "-h") {
                println!("Usage:\n  travel import-offers [--dest <slug>] [--dir <path>] [--files <csv>] [--start <date>] [--end <date>] [--pax N] [--note <text>] [--dry-run]");
                return Ok(());
            }
            let opts = import_offers::parse_args(rest)?;
            import_offers::run(opts).await.map_err(|e| e.to_string())?;
            Ok(())
        }
        [cmd, rest @ ..] if cmd == "add-offer" => {
            add_offer::run(rest).await
        }
        [cmd, rest @ ..] if cmd == "add-besttour-offer" => {
            add_besttour_offer::run(rest).await
        }
        [cmd, rest @ ..] if cmd == "add-lifetour-offer" => {
            add_lifetour_offer::run(rest).await
        }
        [cmd, rest @ ..] if cmd == "import-tour-group-offers" => {
            import_tour_group_offers::run(rest).await
        }
        [cmd, rest @ ..] if cmd == "status" => {
            if rest.iter().any(|a| a == "--help" || a == "-h") {
                println!("Usage:\n  travel status [--full] [--plan-id <id> | --travel-date YYYY-MM-DD]\n  (plan resolution: --plan-id > $TRAVEL_PLAN_ID > --travel-date > active > upcoming > most-recent)");
                return Ok(());
            }
            let full = rest.iter().any(|a| a == "--full");
            status::run(rest, full).await
        }
        [cmd, rest @ ..] if cmd == "set-dates" => {
            if rest.iter().any(|a| a == "--help" || a == "-h") {
                println!("Usage:\n  travel set-dates <start> <end> [reason]");
                return Ok(());
            }
            // Parse: set-dates <start> <end> [reason...]
            if rest.len() < 2 {
                eprintln!("Error: set-dates requires <start> and <end> dates");
                eprintln!("Example: set-dates 2026-02-13 2026-02-17 \"Agent offered Feb 13\"");
                std::process::exit(1);
            }
            let start = rest[0].clone();
            let end = rest[1].clone();
            let reason = if rest.len() > 2 {
                Some(rest[2..].join(" "))
            } else {
                None
            };
            // Resolve plan_id (TRAVEL_PLAN_ID env for now, matching TS CLI)
            let plan_id = env::var("TRAVEL_PLAN_ID").unwrap_or_else(|_| "test-set-dates-2026".to_string());
            set_dates::run(start, end, reason, plan_id).await.map_err(|e| e.to_string())?;
            Ok(())
        }
        [cmd, rest @ ..] if cmd == "update-offer" => {
            if rest.iter().any(|a| a == "--help" || a == "-h") {
                println!("Usage:\n  travel update-offer <offer-id> <date> <availability> [price] [seats] [source]");
                return Ok(());
            }
            let pos: Vec<&String> = rest.iter().filter(|a| !a.starts_with("--")).collect();
            if pos.len() < 3 {
                eprintln!("Error: update-offer requires <offer-id> <date> <availability>");
                eprintln!("Example: update-offer besttour_TYO05MM260211AM 2026-02-13 available 27888 2 agent");
                std::process::exit(1);
            }
            let offer_id = pos[0].clone();
            let date = pos[1].clone();
            let availability = pos[2].clone();
            let price = pos.get(3).and_then(|s| s.parse::<i64>().ok());
            let seats = pos.get(4).and_then(|s| s.parse::<i64>().ok());
            let source_arg = pos.get(5).map(|s| (*s).clone());
            let plan_id =
                env::var("TRAVEL_PLAN_ID").unwrap_or_else(|_| "test-set-dates-2026".to_string());
            update_offer::run(offer_id, date, availability, price, seats, source_arg, plan_id)
                .await
                .map_err(|e| e.to_string())?;
            Ok(())
        }
        [cmd, rest @ ..] if cmd == "select-offer" => {
            if rest.iter().any(|a| a == "--help" || a == "-h") {
                println!("Usage:\n  travel select-offer <offer-id> <date> [--no-populate]");
                return Ok(());
            }
            // Positional args (skip flags): <offer-id> <date>.
            let positional: Vec<&String> =
                rest.iter().filter(|a| !a.starts_with("--")).collect();
            if positional.len() < 2 {
                eprintln!("Error: select-offer requires <offer-id> <date>");
                eprintln!("Example: select-offer besttour_TYO05MM260211AM 2026-02-13");
                std::process::exit(1);
            }
            let offer_id = positional[0].clone();
            let date = positional[1].clone();
            let populate = !rest.iter().any(|a| a == "--no-populate");
            let plan_id =
                env::var("TRAVEL_PLAN_ID").unwrap_or_else(|_| "test-set-dates-2026".to_string());
            cascade::select_offer::run(offer_id, date, populate, plan_id)
                .await
                .map_err(|e| e.to_string())?;
            Ok(())
        }
        [cmd, rest @ ..] if cmd == "set-day-theme" => {
            if rest.iter().any(|a| a == "--help" || a == "-h") {
                println!("Usage:\n  travel set-day-theme <day> [theme] [--zh \"<chinese_title>\"] [--dest <slug>]");
                return Ok(());
            }
            let plan_id = env::var("TRAVEL_PLAN_ID").unwrap_or_else(|_| "test-set-dates-2026".to_string());
            set_day_theme::run(rest, plan_id).await?;
            Ok(())
        }
        [cmd, rest @ ..] if cmd == "set-hotel" => {
            if rest.iter().any(|a| a == "--help" || a == "-h") {
                println!("Usage:\n  travel set-hotel [--dest slug] [--name \"Hotel Name\"] [--check-in YYYY-MM-DD] [--access \"route1 | route2\"] [--note \"...\"]");
                return Ok(());
            }
            let plan_id = env::var("TRAVEL_PLAN_ID").unwrap_or_else(|_| "test-set-dates-2026".to_string());
            set_hotel::run(rest, plan_id).await?;
            Ok(())
        }
        [cmd, rest @ ..] if cmd == "share-token" => {
            if rest.iter().any(|a| a == "--help" || a == "-h") {
                println!("Usage:\n  travel share-token\n  (plan resolved from $TRAVEL_PLAN_ID; mints an opaque per-plan view-scope token for the trip dashboard)");
                return Ok(());
            }
            let plan_id = env::var("TRAVEL_PLAN_ID").unwrap_or_else(|_| "test-set-dates-2026".to_string());
            share_token::run(rest, plan_id).await?;
            Ok(())
        }
        [cmd, rest @ ..] if cmd == "set-flight" => {
            if rest.iter().any(|a| a == "--help" || a == "-h") {
                println!("Usage:\n  travel set-flight <outbound|return> [--dest slug] [--flight SL396] [--airline \"...\"] [--airline-code SL] [--from TPE] [--dep HH:MM] [--dep-terminal T1] [--to KIX] [--arr HH:MM] [--arr-terminal T2] [--date YYYY-MM-DD] [--booked-date YYYY-MM-DD]");
                return Ok(());
            }
            let plan_id = env::var("TRAVEL_PLAN_ID").unwrap_or_else(|_| "test-set-dates-2026".to_string());
            set_flight::run(rest, plan_id).await?;
            Ok(())
        }
        [cmd, rest @ ..] if cmd == "set-airport-transfer" => {
            if rest.iter().any(|a| a == "--help" || a == "-h") {
                println!("Usage:\n  travel set-airport-transfer <arrival|departure> <planned|booked> --selected \"<title|route|...>\" [--candidate \"<...>\"]...");
                return Ok(());
            }
            let plan_id = env::var("TRAVEL_PLAN_ID").unwrap_or_else(|_| "test-set-dates-2026".to_string());
            set_airport_transfer::run(rest, plan_id).await?;
            Ok(())
        }
        [cmd, rest @ ..] if cmd == "set-route-segment" => {
            if rest.iter().any(|a| a == "--help" || a == "-h") {
                println!("Usage:\n  travel set-route-segment <day> <sort_order> <from> <to> <mode> [--duration <min>] [--notes \"...\"] [--start-time HH:MM] [--dest <slug>]");
                return Ok(());
            }
            let plan_id = env::var("TRAVEL_PLAN_ID").unwrap_or_else(|_| "test-set-dates-2026".to_string());
            set_route_segment::run(rest, plan_id).await?;
            Ok(())
        }
        [cmd, rest @ ..] if cmd == "set-route-segments-bulk" => {
            if rest.iter().any(|a| a == "--help" || a == "-h") {
                println!("Usage:\n  travel set-route-segments-bulk <day> --json '[{{...}}]' [--dest <slug>]");
                return Ok(());
            }
            let plan_id = env::var("TRAVEL_PLAN_ID").unwrap_or_else(|_| "test-set-dates-2026".to_string());
            set_route_segment::run_bulk(rest, plan_id).await?;
            Ok(())
        }
        [cmd, rest @ ..] if cmd == "set-tod-focus" || cmd == "set-session-focus" => {
            if rest.iter().any(|a| a == "--help" || a == "-h") {
                println!("Usage:\n  travel set-tod-focus <day> <session> \"<focus_text>\"");
                return Ok(());
            }
            let plan_id = env::var("TRAVEL_PLAN_ID").unwrap_or_else(|_| "test-set-dates-2026".to_string());
            set_tod::run_focus(rest, plan_id).await?;
            Ok(())
        }
        [cmd, rest @ ..] if cmd == "set-tod-time-range" || cmd == "set-session-time-range" => {
            if rest.iter().any(|a| a == "--help" || a == "-h") {
                println!("Usage:\n  travel set-tod-time-range <day> <session> --start HH:MM --end HH:MM [--dest <slug>]");
                return Ok(());
            }
            let plan_id = env::var("TRAVEL_PLAN_ID").unwrap_or_else(|_| "test-set-dates-2026".to_string());
            set_tod::run_time_range(rest, plan_id).await?;
            Ok(())
        }
        [cmd, rest @ ..] if cmd == "set-tod-zh" || cmd == "set-session-zh" => {
            if rest.iter().any(|a| a == "--help" || a == "-h") {
                println!("Usage:\n  travel set-tod-zh <day> <session> [--zh \"...\"] [--transit-zh \"...\"] [--activities-zh-json '[\"...\"]'] [--meals-zh-json '[\"...\"]'] [--dest <slug>]");
                return Ok(());
            }
            let plan_id = env::var("TRAVEL_PLAN_ID").unwrap_or_else(|_| "test-set-dates-2026".to_string());
            set_tod::run_zh(rest, plan_id).await?;
            Ok(())
        }
        [cmd, rest @ ..] if cmd == "set-activity-time" => {
            if rest.iter().any(|a| a == "--help" || a == "-h") {
                println!("Usage:\n  travel set-activity-time <day> <session> <activity> [--start HH:MM] [--end HH:MM] [--fixed true|false] [--dest <slug>]");
                return Ok(());
            }
            let plan_id = env::var("TRAVEL_PLAN_ID").unwrap_or_else(|_| "test-set-dates-2026".to_string());
            set_activity::run_time(rest, plan_id).await?;
            Ok(())
        }
        [cmd, rest @ ..] if cmd == "set-activity-title" => {
            if rest.iter().any(|a| a == "--help" || a == "-h") {
                println!("Usage:\n  travel set-activity-title <day> <session> <activity> <new_title> [--dest <slug>]");
                return Ok(());
            }
            let plan_id = env::var("TRAVEL_PLAN_ID").unwrap_or_else(|_| "test-set-dates-2026".to_string());
            set_activity::run_title(rest, plan_id).await?;
            Ok(())
        }
        [cmd, rest @ ..] if cmd == "set-activity-poi" => {
            if rest.iter().any(|a| a == "--help" || a == "-h") {
                println!("Usage:\n  travel set-activity-poi <day> <session> <poi_id> [--match \"<title substring>\"] [--dest <slug>]");
                return Ok(());
            }
            let plan_id = env::var("TRAVEL_PLAN_ID").unwrap_or_else(|_| "test-set-dates-2026".to_string());
            set_activity_poi::run(rest, plan_id).await?;
            Ok(())
        }
        [cmd, rest @ ..] if cmd == "bookings" => view_bookings::run(rest).await,
        [cmd, rest @ ..] if cmd == "itinerary" => view_itinerary::run(rest).await,
        [cmd, rest @ ..] if cmd == "transport" => view_transport::run(rest).await,
        [group, sub, rest @ ..] if group == "db" && sub == "status" => {
            if !rest.is_empty() {
                return Err(
                    "Usage: travel db status\n  (no arguments; reads Turso via turso-util)"
                        .to_string(),
                );
            }
            db_status::run().await
        }
        [group, sub, rest @ ..] if group == "db" && sub == "exec" => db_exec::run(rest).await,
        [group, sub, rest @ ..] if group == "db" && sub == "query-offers" => {
            let opts = db_query_offers::QueryOffersArgs::parse(rest)?;
            db_query_offers::run(&opts).await
        }
        // ── P2 db subcommands (pre-wired; modules filled per batch) ──
        [group, sub, rest @ ..] if group == "db" && sub == "migrate" => {
            db_migrate::run(rest).await
        }
        [group, sub, action, rest @ ..] if group == "db" && sub == "seed" && action == "plans" => {
            db_seed_plans::run(rest).await
        }
        [group, sub, action, rest @ ..] if group == "db" && sub == "sync" && action == "destinations" => {
            db_sync_destinations::run(rest).await
        }
        [group, sub, action, rest @ ..] if group == "db" && sub == "sync" && action == "events" => {
            db_sync_events::run(rest).await
        }
        [group, sub, action, rest @ ..] if group == "db" && sub == "fetch" && action == "holidays" => {
            db_fetch_holidays::run(rest).await
        }
        [group, sub, rest @ ..] if group == "validate" && sub == "data" => {
            if !rest.is_empty() {
                return Err("Usage: travel validate data\n  (no arguments)".to_string());
            }
            validate::run(validate::Mode::Validate).await
        }
        [cmd] if cmd == "doctor" => validate::run(validate::Mode::Doctor).await,

        // ── P1 Rust-port dispatch (pre-wired; modules filled per batch) ──

        // batch 1: activity mutations (extend set_activity module)
        [cmd, rest @ ..] if cmd == "delete-activity" || cmd == "remove-activity" => {
            if rest.iter().any(|a| a == "--help" || a == "-h") {
                println!("Usage:\n  travel delete-activity <day> <session> <activity_id_or_title> [--dest <slug>]");
                return Ok(());
            }
            let plan_id = env::var("TRAVEL_PLAN_ID").unwrap_or_else(|_| "test-set-dates-2026".to_string());
            set_activity::run_delete(rest, plan_id).await?;
            Ok(())
        }
        [cmd, rest @ ..] if cmd == "set-activity-booking" => {
            if rest.iter().any(|a| a == "--help" || a == "-h") {
                println!("Usage:\n  travel set-activity-booking <day> <session> <activity> <status> [--ref \"...\"] [--book-by YYYY-MM-DD] [--dest <slug>]");
                return Ok(());
            }
            let plan_id = env::var("TRAVEL_PLAN_ID").unwrap_or_else(|_| "test-set-dates-2026".to_string());
            set_activity::run_booking(rest, plan_id).await?;
            Ok(())
        }

        // batch 2: itinerary structure
        [cmd, rest @ ..] if cmd == "scaffold-itinerary" => {
            let plan_id = env::var("TRAVEL_PLAN_ID").unwrap_or_else(|_| "test-set-dates-2026".to_string());
            scaffold_itinerary::run(rest, plan_id).await?;
            Ok(())
        }
        [cmd, rest @ ..] if cmd == "populate-itinerary" => {
            let plan_id = env::var("TRAVEL_PLAN_ID").unwrap_or_else(|_| "test-set-dates-2026".to_string());
            populate_itinerary::run(rest, plan_id).await?;
            Ok(())
        }
        [cmd, rest @ ..] if cmd == "swap-days" => {
            let plan_id = env::var("TRAVEL_PLAN_ID").unwrap_or_else(|_| "test-set-dates-2026".to_string());
            swap_days::run(rest, plan_id).await?;
            Ok(())
        }

        // batch 3: bookings / status
        [cmd, rest @ ..] if cmd == "mark-booked" => {
            let plan_id = env::var("TRAVEL_PLAN_ID").unwrap_or_else(|_| "test-set-dates-2026".to_string());
            mark_booked::run(rest, plan_id).await?;
            Ok(())
        }
        [cmd, rest @ ..] if cmd == "sync-bookings" => {
            let plan_id = env::var("TRAVEL_PLAN_ID").unwrap_or_else(|_| "test-set-dates-2026".to_string());
            sync_bookings::run(rest, plan_id).await?;
            Ok(())
        }
        [cmd, rest @ ..] if cmd == "check-booking-integrity" => {
            let plan_id = env::var("TRAVEL_PLAN_ID").unwrap_or_else(|_| "test-set-dates-2026".to_string());
            booking_integrity::run(rest, plan_id).await?;
            Ok(())
        }
        [cmd, rest @ ..] if cmd == "run-status" => {
            let plan_id = env::var("TRAVEL_PLAN_ID").unwrap_or_else(|_| "test-set-dates-2026".to_string());
            ops::run_status(rest, plan_id).await?;
            Ok(())
        }
        [cmd, rest @ ..] if cmd == "run-list" => {
            let plan_id = env::var("TRAVEL_PLAN_ID").unwrap_or_else(|_| "test-set-dates-2026".to_string());
            ops::run_list(rest, plan_id).await?;
            Ok(())
        }
        [cmd, rest @ ..] if cmd == "validate-itinerary" => {
            let plan_id = env::var("TRAVEL_PLAN_ID").unwrap_or_else(|_| "test-set-dates-2026".to_string());
            validate_itinerary::run(rest, plan_id).await?;
            Ok(())
        }

        // batch 4: shaping + tour-group query
        [cmd, rest @ ..] if cmd == "shaping-init" => shaping::run_init(rest).await,
        [cmd, rest @ ..] if cmd == "shaping-compare" => shaping::run_compare(rest).await,
        [cmd, rest @ ..] if cmd == "shaping-adopt" => shaping::run_adopt(rest).await,
        [cmd, rest @ ..] if cmd == "shaping-baseline" => shaping::run_baseline(rest).await,
        [cmd, rest @ ..] if cmd == "shaping-export" => shaping::run_export(rest).await,
        [cmd, rest @ ..] if cmd == "shaping-import" => shaping::run_import(rest).await,
        [cmd, rest @ ..] if cmd == "query-tour-group-offers" => query_tour_group::run(rest).await,

        // batch 5: weather / prices / compare / chat
        [cmd, rest @ ..] if cmd == "fetch-weather" => {
            let plan_id = env::var("TRAVEL_PLAN_ID").unwrap_or_else(|_| "test-set-dates-2026".to_string());
            weather::run(rest, plan_id).await?;
            Ok(())
        }
        [cmd, rest @ ..] if cmd == "view-prices" => view_prices::run(rest).await,
        [cmd, rest @ ..] if cmd == "compare-offers" => search_compare::run_compare(rest).await,
        [cmd, rest @ ..] if cmd == "search-offers" => search_compare::run_search(rest).await,
        [cmd, rest @ ..] if cmd == "chat-format" => chat_format::run(rest).await,

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
        "Travel CLI\n\nUsage:\n  travel plans\n  travel resolve-plan [--plan-id <id> | --plan-path <path> | --travel-date YYYY-MM-DD | --travel-start YYYY-MM-DD --travel-end YYYY-MM-DD]\n  travel status [--full]\n  travel bookings [--dest slug]\n  travel itinerary [--dest slug]\n  travel transport [--dest slug]\n  travel query-offers [--source a,b] [--region r] [--dest d] [--max-price N] [--start YYYY-MM-DD] [--end YYYY-MM-DD] [--limit N]\n  travel query-destination-ref --slug <destination_slug>\n  travel query-bookings [--trip-id id] [--dest slug] [--category c] [--status s] [--max N]\n  travel check-freshness --source <id> [--region r] [--start YYYY-MM-DD] [--end YYYY-MM-DD] [--max-age N] [--plan-id id] [--dest slug]\n  travel compare trips --trip '<key=value;...>' [--trip '<key=value;...>'] [--market taiwan] [--detailed]\n  travel normalize flights --text '<rendered flight text>' --url '<source url>' [--label name]\n  travel normalize flights --stdin --url '<source url>' [--label name]\n  travel leave calc <start-date> <end-date> [country]\n  travel validate data\n  travel doctor\n\nRules:\n  plain-text input and output; no JSON files or JSON output"
    );
}
