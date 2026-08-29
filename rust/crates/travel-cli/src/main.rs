mod bookings;
mod cascade;
mod catalog_audit; // catalog_runs audit helper for global OTA-catalog mutations
mod checks; // shared lint predicates (single source of truth) — see checks.rs
mod compare;
mod compare_dates;
mod compare_content_depth;
mod compare_true_cost;
mod db;
mod db_exec;
mod db_schema;
mod db_query_offers;
mod db_status;
mod db_token_status;    // db token-status (diagnose Turso credential resolution)
mod destination_ref;
mod flights;
mod flow_decision; // flow-decision — audited stage entry/skip/mode recorder (F6)
mod freshness;
mod import_offers;
mod leave;
mod offers;
mod ota; // rust-first OTA execution (enqueue/claim/parse/write-offers)
mod ota_status; // ota-status DB-native provider coverage view
mod plan;
mod promote_offers;
mod plan_resolver;
mod plans;
mod scrape_parser;
mod set_ota_catalog; // set-ota-source/coverage/region/workflow/url-param catalog mutations
mod set_poi_coords; // set-poi-coords — geocode a destination_pois row (slug-keyed, no audit triad)
mod add_transit; // add-transit — add a destination_transit station pair (slug-keyed, no audit; feeds derive-routes)
mod add_omiyage; // add-omiyage — omiyage item + purchase location (slug-keyed, no audit triad)
mod query_omiyage; // query-omiyage — read-only grouped omiyage view (slug-keyed reference data)
mod omiyage_worklist; // omiyage-worklist — read-only research worklist of omiyage-tagged POIs (writes nothing)
mod set_activity;
mod set_activity_poi;
mod confirm_recommendations;
mod derive_routes;
mod transit_key; // shared destination_transit pair-key math (derive-routes lookup + add-transit write)
mod query_recommendations;
mod set_airport_transfer;
mod set_dates;
mod set_day_theme;
mod set_flight;
mod set_hotel;
mod set_process_status;
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
mod check_hours;        // pre-trip open-hours check
mod shaping;            // batch 4 (shaping-init/compare/adopt/baseline/export/import)
mod shaping_purchase;   // shaping-purchase-matrix — read-only purchase decision matrix
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
mod db_seed_destination_refs; // db seed destination-refs (scripts/seed-destination-refs.ts)
mod db_seed_ota_knowledge;    // db seed ota-knowledge (scripts/seed-ota-knowledge.ts)
mod db_seed_test_plan;        // db seed test-plan (scripts/seed-test-plan.ts)
mod db_sync_destinations; // db sync destinations (scripts/turso-sync-destinations.ts)
mod db_sync_events;     // db sync events (scripts/turso-sync-events.ts)
mod db_fetch_holidays;  // db fetch holidays (scripts/fetch-taiwan-holidays.ts)
mod create_plan;        // create-plan (fast-path plan seed)
mod mark_plan_deleted;  // mark-plan-deleted (soft-delete a plan)
mod set_plan_name;      // set-plan-name (rename plan_destinations.display_name)
mod set_active_destination; // set-active-destination (switch plan_metadata.active_destination)
mod db_cleanup_deleted; // db cleanup-deleted (batched hard-wipe of soft-deleted plans)
mod mark_maps_snapshotted; // mark-maps-snapshotted (stamp dashboard map snapshot time)
mod check_maps_fresh;   // check-maps-fresh (map-snapshot staleness lint)
mod snapshot_maps;      // snapshot-maps (wrap scripts/snapshot-maps.sh: capture+upload route maps)

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
        [group, sub, rest @ ..] if group == "compare" && sub == "content-depth" => {
            if rest.iter().any(|a| a == "--help" || a == "-h") {
                println!("Usage:\n  travel compare content-depth --plan-id <drill> [--against <ref>]\n  (--against default: okinawa-2026; read-only depth oracle for the drill loop)");
                return Ok(());
            }
            compare_content_depth::run(rest).await
        }
        [group, sub, rest @ ..] if group == "normalize" && sub == "flights" => {
            normalize_flights(rest)
        }
        [cmd] if cmd == "plans" || cmd == "list-plans" => plans::run().await,
        [cmd, rest @ ..] if cmd == "create-plan" => {
            if wants_help(
                rest,
                "travel create-plan <plan_id> --dest <slug> --start YYYY-MM-DD --end YYYY-MM-DD --airport <IATA> [--region <name>] [--display-name <name>] [--origin <code>] [--nights N]\n  Create a fast-path plan (plans + metadata + date_anchors + the process ladder) so set-flight/set-hotel/itinerary work. Dest must be registered (/new-destination). Dates-inclusive — no separate set-dates needed.",
            ) {
                return Ok(());
            }
            create_plan::run(rest, String::new()).await
        }
        [cmd, rest @ ..] if cmd == "mark-plan-deleted" => {
            // The target plan_id is a REQUIRED positional — do NOT route through
            // the resolver ladder (which would pick a default plan if omitted).
            // The command parses/validates the explicit target itself.
            mark_plan_deleted::run(rest, String::new()).await
        }
        [cmd, rest @ ..] if cmd == "resolve-plan" => plan_resolver::run_cli(rest).await,
        [cmd, rest @ ..] if cmd == "query-offers" => {
            if wants_help(rest, "travel query-offers [--dest <slug>] [--region <r>] [--start <date>] [--end <date>] [--source <s>] [--max-price <twd>] [--limit <n>] [--capture-id <id>] [--job-id <id>] [--attempt-id <id>]") { return Ok(()); }
            let opts = offers::OffersArgs::parse(rest)?;
            offers::run(&opts).await
        }
        [cmd, rest @ ..] if cmd == "query-destination-ref" || cmd == "destination-ref" => {
            if wants_help(rest, "travel query-destination-ref --slug <destination_slug>\n  (lists areas, clusters, POIs, transit, tips for a registered destination — e.g. tokyo_2026)") { return Ok(()); }
            let opts = destination_ref::DestRefArgs::parse(rest)?;
            destination_ref::run(&opts).await
        }
        [cmd, rest @ ..] if cmd == "query-bookings" => {
            if wants_help(rest, "travel query-bookings [--dest <slug>] [--category <c>] [--status <s>] [--max <n>] [--trip-id <id>]") { return Ok(()); }
            let opts = bookings::QueryBookingsArgs::parse(rest)?;
            bookings::run(&opts).await
        }
        [cmd, rest @ ..] if cmd == "check-freshness" => {
            if wants_help(rest, "travel check-freshness --source <s> [--dest <slug>] [--region <r>] [--start <date>] [--end <date>] [--max-age <hours>] [--plan-id <id>]") { return Ok(()); }
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
        [cmd, rest @ ..] if cmd == "promote-offers" => {
            if rest.iter().any(|a| a == "--help" || a == "-h") {
                println!("Usage:\n  travel promote-offers --from-offers --dest <slug> [--plan-id <id>] [--source <id>] [--start <date>] [--end <date>] [--pax N] [--dry-run]");
                return Ok(());
            }
            let opts = promote_offers::parse_args(rest)?;
            promote_offers::run(opts).await.map_err(|e| e.to_string())?;
            Ok(())
        }
        [cmd, rest @ ..] if cmd == "flow-decision" => {
            if rest.iter().any(|a| a == "--help" || a == "-h") {
                println!(
                    "Usage:\n  travel flow-decision <stage> <decision> [--mode <m>] [--reason <r>] [--source <s>] [--plan-id <id>]"
                );
                return Ok(());
            }
            flow_decision::run(rest).await
        }
        [cmd, rest @ ..] if cmd == "set-ota-source" => {
            set_ota_catalog::run_set_source(rest).await
        }
        [cmd, rest @ ..] if cmd == "set-ota-coverage" => {
            set_ota_catalog::run_set_coverage(rest).await
        }
        [cmd, rest @ ..] if cmd == "set-ota-region" => {
            set_ota_catalog::run_set_region(rest).await
        }
        [cmd, rest @ ..] if cmd == "set-ota-workflow" => {
            set_ota_catalog::run_set_workflow(rest).await
        }
        [cmd, rest @ ..] if cmd == "set-ota-url-param" => {
            set_ota_catalog::run_set_url_param(rest).await
        }
        [cmd, rest @ ..] if cmd == "set-poi-coords" => {
            set_poi_coords::run(rest).await
        }
        [cmd, rest @ ..] if cmd == "add-transit" => {
            add_transit::run(rest).await
        }
        [cmd, rest @ ..] if cmd == "add-omiyage" => {
            add_omiyage::run(rest).await
        }
        [cmd, rest @ ..] if cmd == "query-omiyage" => {
            query_omiyage::run(rest).await
        }
        [cmd, rest @ ..] if cmd == "omiyage-worklist" => {
            omiyage_worklist::run(rest).await
        }
        [cmd, rest @ ..] if cmd == "ota-status" => {
            ota_status::run(rest).await
        }
        [group, sub, rest @ ..] if group == "ota" => ota::dispatch(sub, rest).await,
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
                return Err("Error: set-dates requires <start> and <end> dates\n\
                            Example: set-dates 2026-02-13 2026-02-17 \"Agent offered Feb 13\""
                    .to_string());
            }
            let start = rest[0].clone();
            let end = rest[1].clone();
            let reason = if rest.len() > 2 {
                Some(rest[2..].join(" "))
            } else {
                None
            };
            // Resolve plan_id (TRAVEL_PLAN_ID env for now, matching TS CLI)
            let plan_id = plan_resolver::resolve_plan_id(rest).await?;
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
                return Err("Error: update-offer requires <offer-id> <date> <availability>\n\
                            Example: update-offer besttour_TYO05MM260211AM 2026-02-13 available 27888 2 agent"
                    .to_string());
            }
            let offer_id = pos[0].clone();
            let date = pos[1].clone();
            let availability = pos[2].clone();
            let price = pos.get(3).and_then(|s| s.parse::<i64>().ok());
            let seats = pos.get(4).and_then(|s| s.parse::<i64>().ok());
            let source_arg = pos.get(5).map(|s| (*s).clone());
            let plan_id = plan_resolver::resolve_plan_id(rest).await?;
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
                return Err("Error: select-offer requires <offer-id> <date>\n\
                            Example: select-offer besttour_TYO05MM260211AM 2026-02-13"
                    .to_string());
            }
            let offer_id = positional[0].clone();
            let date = positional[1].clone();
            let populate = !rest.iter().any(|a| a == "--no-populate");
            let plan_id = plan_resolver::resolve_plan_id(rest).await?;
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
            let plan_id = plan_resolver::resolve_plan_id(rest).await?;
            set_day_theme::run(rest, plan_id).await?;
            Ok(())
        }
        [cmd, rest @ ..] if cmd == "set-hotel" => {
            if rest.iter().any(|a| a == "--help" || a == "-h") {
                println!("Usage:\n  travel set-hotel [--dest slug] [--name \"Hotel Name\"] [--check-in YYYY-MM-DD] [--access \"route1 | route2\"] [--note \"...\"]");
                return Ok(());
            }
            let plan_id = plan_resolver::resolve_plan_id(rest).await?;
            set_hotel::run(rest, plan_id).await?;
            Ok(())
        }
        [cmd, rest @ ..] if cmd == "set-plan-name" => {
            if wants_help(
                rest,
                "travel set-plan-name <name> [--dest <slug>] [--plan-id <id> | --travel-date ...]\n  Rename a plan's display label (plan_destinations.display_name). --dest disambiguates a multi-destination plan.",
            ) {
                return Ok(());
            }
            let plan_id = plan_resolver::resolve_plan_id(rest).await?;
            set_plan_name::run(rest, plan_id).await?;
            Ok(())
        }
        [cmd, rest @ ..] if cmd == "set-active-destination" => {
            if wants_help(
                rest,
                "travel set-active-destination <slug> [--plan-id <id> | --travel-date ...]\n  Switch plan_metadata.active_destination to one of the plan's destinations.",
            ) {
                return Ok(());
            }
            let plan_id = plan_resolver::resolve_plan_id(rest).await?;
            set_active_destination::run(rest, plan_id).await?;
            Ok(())
        }
        [cmd, rest @ ..] if cmd == "share-token" => {
            if rest.iter().any(|a| a == "--help" || a == "-h") {
                println!("Usage:\n  travel share-token                         mint a new per-plan view-scope token + print its dashboard URL\n  travel share-token --show                  list token fingerprints/status (read-only)\n  travel share-token --show-full             list full token URLs (sensitive)\n  travel share-token deactivate <token>      deactivate an active token\n  (plan resolved by --plan-id / $TRAVEL_PLAN_ID / --dest/date fallbacks; URL host overridable via TRAVEL_DASHBOARD_HOST)");
                return Ok(());
            }
            let plan_id = plan_resolver::resolve_plan_id(rest).await?;
            share_token::run(rest, plan_id).await?;
            Ok(())
        }
        [cmd, rest @ ..] if cmd == "set-flight" => {
            if rest.iter().any(|a| a == "--help" || a == "-h") {
                println!("Usage:\n  travel set-flight <outbound|return> [--dest slug] [--flight SL396] [--airline \"...\"] [--airline-code SL] [--from TPE] [--dep HH:MM] [--dep-terminal T1] [--to KIX] [--arr HH:MM] [--arr-terminal T2] [--date YYYY-MM-DD] [--booked-date YYYY-MM-DD]");
                return Ok(());
            }
            let plan_id = plan_resolver::resolve_plan_id(rest).await?;
            set_flight::run(rest, plan_id).await?;
            Ok(())
        }
        [cmd, rest @ ..] if cmd == "set-process-status" => {
            if rest.iter().any(|a| a == "--help" || a == "-h") {
                println!("Usage:\n  travel set-process-status <process_id> <target_status> [--dest <slug>] [--plan-id <id>]");
                return Ok(());
            }
            let plan_id = plan_resolver::resolve_plan_id(rest).await?;
            set_process_status::run(rest, plan_id).await?;
            Ok(())
        }
        [cmd, rest @ ..] if cmd == "set-airport-transfer" => {
            if rest.iter().any(|a| a == "--help" || a == "-h") {
                println!("Usage:\n  travel set-airport-transfer <arrival|departure> <planned|booked> --selected \"<title|route|...>\" [--candidate \"<...>\"]...");
                return Ok(());
            }
            let plan_id = plan_resolver::resolve_plan_id(rest).await?;
            set_airport_transfer::run(rest, plan_id).await?;
            Ok(())
        }
        [cmd, rest @ ..] if cmd == "set-route-segment" => {
            if rest.iter().any(|a| a == "--help" || a == "-h") {
                println!("Usage:\n  travel set-route-segment <day> <sort_order> <from> <to> <mode> [--duration N] [--notes \"...\"] [--start-time HH:MM] [--recommended] [--dest <slug>]\n  (one segment; use set-route-segments-bulk for a whole day. --recommended marks it AI-recommended/unconfirmed. Keep stop names clean — no （…）notes or clock times inside the stop.)");
                return Ok(());
            }
            let plan_id = plan_resolver::resolve_plan_id(rest).await?;
            set_route_segment::run(rest, plan_id).await?;
            Ok(())
        }
        [cmd, rest @ ..] if cmd == "set-route-segments-bulk" => {
            if rest.iter().any(|a| a == "--help" || a == "-h") {
                println!("Usage:\n  travel set-route-segments-bulk <day> --seg \"from|to|mode[|duration[|start_time[|notes]]]\" [--seg ...] [--recommended] [--dest <slug>]\n  (--recommended marks every segment AI-recommended/unconfirmed)");
                return Ok(());
            }
            let plan_id = plan_resolver::resolve_plan_id(rest).await?;
            set_route_segment::run_bulk(rest, plan_id).await?;
            Ok(())
        }
        [cmd, rest @ ..] if cmd == "set-tod-focus" || cmd == "set-session-focus" => {
            if rest.iter().any(|a| a == "--help" || a == "-h") {
                println!("Usage:\n  travel set-tod-focus <day> <session> \"<focus_text>\" [--zh \"<chinese focus>\"] [--dest slug]\n  (--zh sets focus_zh too — the dashboard renders ZH by default)");
                return Ok(());
            }
            let plan_id = plan_resolver::resolve_plan_id(rest).await?;
            set_tod::run_focus(rest, plan_id).await?;
            Ok(())
        }
        [cmd, rest @ ..] if cmd == "set-tod-time-range" || cmd == "set-session-time-range" => {
            if rest.iter().any(|a| a == "--help" || a == "-h") {
                println!("Usage:\n  travel set-tod-time-range <day> <session> --start HH:MM --end HH:MM [--dest <slug>]");
                return Ok(());
            }
            let plan_id = plan_resolver::resolve_plan_id(rest).await?;
            set_tod::run_time_range(rest, plan_id).await?;
            Ok(())
        }
        [cmd, rest @ ..] if cmd == "set-tod-zh" || cmd == "set-session-zh" => {
            if rest.iter().any(|a| a == "--help" || a == "-h") {
                println!("Usage:\n  travel set-tod-zh <day> <session> [--zh \"...\"] [--transit-zh \"...\"] [--activity-zh \"...\" (repeatable)] [--clear-activities] [--dest <slug>]");
                return Ok(());
            }
            let plan_id = plan_resolver::resolve_plan_id(rest).await?;
            set_tod::run_zh(rest, plan_id).await?;
            Ok(())
        }
        [cmd, rest @ ..] if cmd == "set-meals" => {
            if rest.iter().any(|a| a == "--help" || a == "-h") {
                println!("Usage:\n  travel set-meals <day> <session> --meal \"<text>\" [--meal \"<text>\"...] [--recommended] [--dest <slug>]\n  (a meal may carry a map pin: \"<label>｜map:<query>\"; --recommended marks it AI-recommended/unconfirmed)");
                return Ok(());
            }
            let plan_id = plan_resolver::resolve_plan_id(rest).await?;
            set_tod::run_meals(rest, plan_id).await?;
            Ok(())
        }
        [cmd, rest @ ..] if cmd == "set-activity-time" => {
            if rest.iter().any(|a| a == "--help" || a == "-h") {
                println!("Usage:\n  travel set-activity-time <day> <session> <activity> [--start HH:MM] [--end HH:MM] [--fixed true|false] [--dest <slug>]");
                return Ok(());
            }
            let plan_id = plan_resolver::resolve_plan_id(rest).await?;
            set_activity::run_time(rest, plan_id).await?;
            Ok(())
        }
        [cmd, rest @ ..] if cmd == "set-activity-title" => {
            if rest.iter().any(|a| a == "--help" || a == "-h") {
                println!("Usage:\n  travel set-activity-title <day> <session> <activity> <new_title> [--dest <slug>]");
                return Ok(());
            }
            let plan_id = plan_resolver::resolve_plan_id(rest).await?;
            set_activity::run_title(rest, plan_id).await?;
            Ok(())
        }
        [cmd, rest @ ..] if cmd == "set-activity-poi" => {
            if rest.iter().any(|a| a == "--help" || a == "-h") {
                println!("Usage:\n  travel set-activity-poi <day> <session> <poi_id> [--match \"<title substring>\"] [--dest <slug>]");
                return Ok(());
            }
            let plan_id = plan_resolver::resolve_plan_id(rest).await?;
            set_activity_poi::run(rest, plan_id).await?;
            Ok(())
        }
        [cmd, rest @ ..] if cmd == "add-activity" => {
            if rest.iter().any(|a| a == "--help" || a == "-h") {
                println!("Usage:\n  travel add-activity <day> <session> <title> [--after <id|title>] [--recommended] [--area ..] [--station ..] [--duration MIN] [--start HH:MM] [--end HH:MM] [--fixed true|false] [--priority must|want|optional] [--notes ..] [--dest <slug>]\n  (--recommended marks it AI-recommended/unconfirmed)");
                return Ok(());
            }
            let plan_id = plan_resolver::resolve_plan_id(rest).await?;
            set_activity::run_add(rest, plan_id).await?;
            Ok(())
        }
        [cmd, rest @ ..] if cmd == "move-activity" => {
            if wants_help(rest, "travel move-activity <day> <from-session> <to-session> <id|title> [--to-day N] [--dest slug]\n  (move an activity to another session/day, preserving its id + poi link; appended at the end of the target session)") { return Ok(()); }
            let plan_id = plan_resolver::resolve_plan_id(rest).await?;
            set_activity::run_move(rest, plan_id).await?;
            Ok(())
        }
        [cmd, rest @ ..] if cmd == "reorder-activities" => {
            if rest.iter().any(|a| a == "--help" || a == "-h") {
                println!("Usage:\n  travel reorder-activities <day> <session> <id-or-title> <id-or-title> ... [--dest <slug>]\n  (list ALL activities in the session, in the desired order)");
                return Ok(());
            }
            let plan_id = plan_resolver::resolve_plan_id(rest).await?;
            set_activity::run_reorder(rest, plan_id).await?;
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
        [group, sub, rest @ ..] if group == "db" && sub == "token-status" => {
            db_token_status::run(rest).await
        }
        [group, sub, rest @ ..] if group == "db" && sub == "exec" => {
            if wants_help(rest, "travel db exec \"<SQL>\"  (run one SQL statement; rows print as plain text)") {
                return Ok(());
            }
            db_exec::run(rest).await
        }
        [group, sub, rest @ ..] if group == "db" && sub == "schema" => db_schema::run(rest).await,
        [group, sub, rest @ ..] if group == "db" && sub == "cleanup-deleted" => {
            db_cleanup_deleted::run(rest).await
        }
        [group, sub, rest @ ..] if group == "db" && sub == "query-offers" => {
            let opts = db_query_offers::QueryOffersArgs::parse(rest)?;
            db_query_offers::run(&opts).await
        }
        // ── P2 db subcommands (pre-wired; modules filled per batch) ──
        [group, sub, rest @ ..] if group == "db" && sub == "migrate" => {
            // --help must NOT run the migration (it's a schema WRITE).
            if wants_help(rest, "travel db migrate  (create/upgrade tables — idempotent; runs a schema WRITE)") {
                return Ok(());
            }
            db_migrate::run(rest).await
        }
        [group, sub, action, rest @ ..] if group == "db" && sub == "seed" && action == "plans" => {
            if wants_help(rest, "travel db seed plans  (one-time plan seed — a WRITE)") {
                return Ok(());
            }
            db_seed_plans::run(rest).await
        }
        [group, sub, action, rest @ ..] if group == "db" && sub == "seed" && action == "destination-refs" => {
            if wants_help(rest, "travel db seed destination-refs  (seed destination reference data — a WRITE)") {
                return Ok(());
            }
            db_seed_destination_refs::run(rest).await
        }
        [group, sub, action, rest @ ..] if group == "db" && sub == "seed" && action == "ota-knowledge" => {
            if wants_help(rest, "travel db seed ota-knowledge  (seed OTA knowledge tables — a WRITE)") {
                return Ok(());
            }
            db_seed_ota_knowledge::run(rest).await
        }
        [group, sub, action, rest @ ..] if group == "db" && sub == "seed" && action == "test-plan" => {
            if wants_help(rest, "travel db seed test-plan [source] [target]  (clone a plan into a throwaway test plan — a WRITE)") {
                return Ok(());
            }
            db_seed_test_plan::run(rest).await
        }
        [group, sub, action, rest @ ..] if group == "db" && sub == "sync" && action == "destinations" => {
            if wants_help(rest, "travel db sync destinations  (sync destination_config from reference data — a WRITE)") {
                return Ok(());
            }
            db_sync_destinations::run(rest).await
        }
        [group, sub, action, rest @ ..] if group == "db" && sub == "sync" && action == "events" => {
            if wants_help(rest, "travel db sync events  (sync the event log from data/state.json — a WRITE)") {
                return Ok(());
            }
            db_sync_events::run(rest).await
        }
        [group, sub, action, rest @ ..] if group == "db" && sub == "fetch" && action == "holidays" => {
            if wants_help(rest, "travel db fetch holidays [url]  (fetch + store the Taiwan holiday calendar — a WRITE)") {
                return Ok(());
            }
            db_fetch_holidays::run(rest).await
        }
        [group, sub, rest @ ..] if group == "validate" && sub == "publish" => {
            let plan_id = plan_resolver::resolve_plan_id(rest).await?;
            let dest = parse_flag_value(rest, "--dest");
            validate::run(validate::Mode::Publish { plan_id, dest }).await
        }
        [group, sub, rest @ ..] if group == "validate" && sub == "data" => {
            if !rest.is_empty() {
                return Err("Usage: travel validate data\n  (no arguments)".to_string());
            }
            validate::run(validate::Mode::Validate).await
        }
        [cmd] if cmd == "doctor" => validate::run(validate::Mode::Doctor).await,

        // Map-snapshot staleness lint + its timestamp-recording companion.
        [cmd, rest @ ..] if cmd == "mark-maps-snapshotted" => {
            let plan_id = plan_resolver::resolve_plan_id(rest).await.unwrap_or_default();
            mark_maps_snapshotted::run(rest, plan_id).await?;
            Ok(())
        }
        [cmd, rest @ ..] if cmd == "check-maps-fresh" => {
            check_maps_fresh::run(rest).await?;
            Ok(())
        }
        [cmd, rest @ ..] if cmd == "snapshot-maps" => {
            let plan_id = plan_resolver::resolve_plan_id(rest).await.unwrap_or_default();
            snapshot_maps::run(rest, plan_id).await?;
            Ok(())
        }

        // ── P1 Rust-port dispatch (pre-wired; modules filled per batch) ──

        // batch 1: activity mutations (extend set_activity module)
        [cmd, rest @ ..] if cmd == "delete-activity" || cmd == "remove-activity" => {
            if rest.iter().any(|a| a == "--help" || a == "-h") {
                println!("Usage:\n  travel delete-activity <day> <session> <activity_id_or_title> [--dest <slug>]");
                return Ok(());
            }
            let plan_id = plan_resolver::resolve_plan_id(rest).await?;
            set_activity::run_delete(rest, plan_id).await?;
            Ok(())
        }
        [cmd, rest @ ..] if cmd == "set-activity-booking" => {
            if rest.iter().any(|a| a == "--help" || a == "-h") {
                println!("Usage:\n  travel set-activity-booking <day> <session> <activity> <status> [--ref \"...\"] [--book-by YYYY-MM-DD] [--dest <slug>]");
                return Ok(());
            }
            let plan_id = plan_resolver::resolve_plan_id(rest).await?;
            set_activity::run_booking(rest, plan_id).await?;
            Ok(())
        }

        // batch 2: itinerary structure
        [cmd, rest @ ..] if cmd == "scaffold-itinerary" => {
            if wants_help(rest, "travel scaffold-itinerary [--dest slug]\n  (create empty day/session rows for the plan's date range)") { return Ok(()); }
            let plan_id = plan_resolver::resolve_plan_id(rest).await?;
            scaffold_itinerary::run(rest, plan_id).await?;
            Ok(())
        }
        [cmd, rest @ ..] if cmd == "populate-itinerary" => {
            if wants_help(rest, "travel populate-itinerary [--dest slug]\n  (fill scaffolded days from destination reference data)") { return Ok(()); }
            let plan_id = plan_resolver::resolve_plan_id(rest).await?;
            populate_itinerary::run(rest, plan_id).await?;
            Ok(())
        }
        [cmd, rest @ ..] if cmd == "swap-days" => {
            if wants_help(rest, "travel swap-days <dayA> <dayB> [--dest slug]\n  (swap the content of two days; date/day_number/day_type preserved)") { return Ok(()); }
            let plan_id = plan_resolver::resolve_plan_id(rest).await?;
            swap_days::run(rest, plan_id).await?;
            Ok(())
        }

        // batch 3: bookings / status
        [cmd, rest @ ..] if cmd == "mark-booked" => {
            // Reject unknown flags BEFORE resolve_plan_id opens Turso — else a
            // typo'd `--dry-run` is ignored (dry_run=false) and this commits a
            // real booking transition the user meant to preview.
            plan_resolver::reject_unknown_flags(
                rest,
                &["--dest", "--plan-id", "--plan-path", "--travel-date", "--travel-start", "--travel-end"],
                &["--dry-run"],
            )?;
            let plan_id = plan_resolver::resolve_plan_id(rest).await?;
            mark_booked::run(rest, plan_id).await?;
            Ok(())
        }
        [cmd, rest @ ..] if cmd == "sync-bookings" => {
            // Reject unknown flags before the resolver opens Turso (a typo'd
            // --dry-run would otherwise run a real sync).
            plan_resolver::reject_unknown_flags(
                rest,
                &["--plan-id", "--plan-path", "--travel-date", "--travel-start", "--travel-end", "--trip-id"],
                &["--dry-run"],
            )?;
            let plan_id = plan_resolver::resolve_plan_id(rest).await?;
            sync_bookings::run(rest, plan_id).await?;
            Ok(())
        }
        [cmd, rest @ ..] if cmd == "check-booking-integrity" => {
            let plan_id = plan_resolver::resolve_plan_id(rest).await?;
            booking_integrity::run(rest, plan_id).await?;
            Ok(())
        }
        [cmd, rest @ ..] if cmd == "run-status" => {
            let plan_id = plan_resolver::resolve_plan_id(rest).await?;
            ops::run_status(rest, plan_id).await?;
            Ok(())
        }
        [cmd, rest @ ..] if cmd == "run-list" => {
            let plan_id = plan_resolver::resolve_plan_id(rest).await?;
            ops::run_list(rest, plan_id).await?;
            Ok(())
        }
        [cmd, rest @ ..] if cmd == "validate-itinerary" => {
            if wants_help(rest, "travel validate-itinerary [--dest slug] [--severity error|warn|info]\n  (lint the daily itinerary: map links, open hours, reservations)") { return Ok(()); }
            let plan_id = plan_resolver::resolve_plan_id(rest).await?;
            validate_itinerary::run(rest, plan_id).await?;
            Ok(())
        }
        [cmd, rest @ ..] if cmd == "check-hours" => {
            if wants_help(rest, "travel check-hours [--dest slug]\n  (flag activities scheduled outside their POI open hours)") { return Ok(()); }
            let plan_id = plan_resolver::resolve_plan_id(rest).await?;
            check_hours::run(rest, plan_id).await?;
            Ok(())
        }

        // batch 4: shaping + tour-group query
        [cmd, rest @ ..] if cmd == "shaping-init" => shaping::run_init(rest).await,
        [cmd, rest @ ..] if cmd == "shaping-compare" => shaping::run_compare(rest).await,
        [cmd, rest @ ..] if cmd == "shaping-adopt" => shaping::run_adopt(rest).await,
        [cmd, rest @ ..] if cmd == "shaping-baseline" => shaping::run_baseline(rest).await,
        [cmd, rest @ ..] if cmd == "shaping-purchase-matrix" => shaping_purchase::run(rest).await,
        [cmd, rest @ ..] if cmd == "shaping-export" => shaping::run_export(rest).await,
        [cmd, rest @ ..] if cmd == "shaping-import" => shaping::run_import(rest).await,
        [cmd, rest @ ..] if cmd == "query-tour-group-offers" => query_tour_group::run(rest).await,

        // batch 5: weather / prices / compare / chat
        [cmd, rest @ ..] if cmd == "fetch-weather" => {
            if wants_help(rest, "travel fetch-weather [--dest slug] [--all]\n  (fetch Open-Meteo forecast into the day rows)") { return Ok(()); }
            // Reject unknown flags before the resolver opens Turso (a typo'd
            // --all/--dest would otherwise be silently ignored).
            plan_resolver::reject_unknown_flags(
                rest,
                &["--dest", "--plan-id", "--plan-path", "--travel-date", "--travel-start", "--travel-end"],
                &["--all"],
            )?;
            let plan_id = plan_resolver::resolve_plan_id(rest).await?;
            weather::run(rest, plan_id).await?;
            Ok(())
        }
        [cmd, rest @ ..] if cmd == "view-prices" => view_prices::run(rest).await,
        [cmd, rest @ ..] if cmd == "compare-offers" => search_compare::run_compare(rest).await,
        [cmd, rest @ ..] if cmd == "search-offers" => search_compare::run_search(rest).await,
        [cmd, rest @ ..] if cmd == "chat-format" => chat_format::run(rest).await,
        [cmd, rest @ ..] if cmd == "confirm-recommendations" => {
            if wants_help(
                rest,
                "travel confirm-recommendations [--day N] [--session morning|noon|afternoon|evening] [--kind activity|meal|route] [--dest <slug>]\n  Flips ai_recommended itinerary content to confirmed, scoped by the filters.\n  Note: --session scopes activities/meals only — routes have no session, so they are confirmed by --day (or all days). Use `query-recommendations` with the same flags to preview first.",
            ) {
                return Ok(());
            }
            let plan_id = plan_resolver::resolve_plan_id(rest).await?;
            confirm_recommendations::run(rest, plan_id).await?;
            Ok(())
        }
        [cmd, rest @ ..] if cmd == "derive-routes" => {
            if wants_help(
                rest,
                "travel derive-routes [--day N] [--dest <slug>]\n  Derive ai_recommended transit route segments from consecutive activity stations. Skips days with confirmed route segments; idempotent.",
            ) {
                return Ok(());
            }
            let plan_id = plan_resolver::resolve_plan_id(rest).await?;
            derive_routes::run(rest, plan_id).await?;
            Ok(())
        }
        [cmd, rest @ ..] if cmd == "query-recommendations" => {
            if wants_help(
                rest,
                "travel query-recommendations [--day N] [--session morning|noon|afternoon|evening] [--kind activity|meal|route] [--dest <slug>]\n  Lists ai_recommended itinerary content awaiting confirmation (read-only).",
            ) {
                return Ok(());
            }
            let plan_id = plan_resolver::resolve_plan_id(rest).await?;
            query_recommendations::run(rest, plan_id).await?;
            Ok(())
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

/// If `rest` contains `--help`/`-h`, print `usage` and return true (caller
/// should stop). Lets simple dispatch arms get a one-line help without a bespoke
/// block each. Returns false when no help flag is present.
fn wants_help(rest: &[String], usage: &str) -> bool {
    if rest.iter().any(|a| a == "--help" || a == "-h") {
        println!("Usage:\n  {usage}");
        true
    } else {
        false
    }
}

fn parse_flag_value(args: &[String], name: &str) -> Option<String> {
    let mut i = 0;
    while i < args.len() {
        if args[i] == name {
            return args.get(i + 1).cloned();
        }
        i += 1;
    }
    None
}

fn print_usage() {
    // Grouped command reference. Run `travel <cmd> --help` for a command's args.
    // Keep this in sync when adding a dispatch arm — it is the only discovery
    // surface for the full command set.
    println!(
        "Travel CLI — plain-text in/out, no JSON. Run `travel <cmd> --help` for details.\n\
\n\
VIEWS\n\
  plans                         list DB plans and date anchors\n\
  status [--full]               booking + process overview\n\
  itinerary [--dest slug]       daily plan\n\
  transport [--dest slug]       transport summary\n\
  bookings [--dest slug]        booking ledger\n\
  query-bookings [--dest slug] [--category c] [--status s] [--max N]\n\
  query-offers [--source a,b] [--dest d] [--max-price N] [--start D] [--end D] [--limit N] [--capture-id|--job-id|--attempt-id]\n\
  query-recommendations [--day N] [--session s] [--kind activity|meal|route] [--dest slug]  List AI-recommended items awaiting confirmation\n\
  query-destination-ref --slug <slug>\n\
  query-omiyage --slug <slug>     omiyage items + purchase locations (slug-keyed)\n\
  omiyage-worklist --slug <slug>   omiyage-tagged POIs as research worklist (read-only; writes nothing)\n\
  view-prices | check-freshness --source <id> [--dest slug]\n\
\n\
ITINERARY EDITS (mutations — audited; most take [--dest slug])\n\
  create-plan <plan_id> --dest <slug> --start <d> --end <d> --airport <IATA> [--region ..] [--nights N]  Create a fast-path plan\n\
  set-dates <start> <end> [reason]\n\
  set-day-theme <day> [theme] [--zh \"<zh>\"]\n\
  scaffold-itinerary | populate-itinerary | swap-days <dayA> <dayB>\n\
  add-activity <day> <session> <title> [--after <id|title>] [--zh \"<zh>\"] [...]\n\
  set-activity-title <day> <session> <activity> <new_title> [--zh \"<zh>\"]\n\
  set-activity-time | set-activity-poi | set-activity-booking | delete-activity\n\
  reorder-activities <day> <session> <id|title> ...\n\
  move-activity <day> <from-session> <to-session> <id|title> [--to-day N]\n\
  set-meals <day> <session> --meal \"<text>\" [--meal ...] [--zh \"<zh>\" ...]\n\
  confirm-recommendations [--day N] [--session s] [--kind activity|meal|route]  Flip AI-recommended → confirmed\n\
  derive-routes [--day N] [--dest <slug>]  Derive ai_recommended transit route segments from activity stations\n\
  set-tod-focus | set-tod-time-range | set-tod-zh <day> <session> [...]\n\
  set-route-segment | set-route-segments-bulk <day> --seg \"from|to|mode[|...]\"\n\
  set-flight | set-hotel | set-airport-transfer | mark-booked | sync-bookings\n\
\n\
SHOP / OFFERS\n\
  import-offers [--dest slug] [--dir path] [--dry-run]\n\
  add-offer | add-besttour-offer | add-lifetour-offer | update-offer | select-offer\n\
  shaping-init | shaping-compare | shaping-adopt | shaping-baseline | shaping-export | shaping-import\n\
  query-tour-group-offers | import-tour-group-offers | compare-offers | search-offers\n\
  ota {{enqueue|claim|heartbeat|finish|reap-stale|run|write-offers|observations|show-capture}}\n\
\n\
VALIDATE / CHECKS\n\
  validate data | validate publish | doctor | validate-itinerary | check-hours\n\
  check-booking-integrity | check-maps-fresh | mark-maps-snapshotted | snapshot-maps\n\
  set-poi-coords <slug> <poi_id> <lat> <lon>  (geocode a POI; global/slug-keyed, no --plan-id)\n\
  add-transit | add-omiyage | query-omiyage | omiyage-worklist  (slug-keyed reference data; no --plan-id)\n\
  run-status | run-list | resolve-plan [--plan-id|--travel-date ...]\n\
\n\
COMPARE / UTIL\n\
  compare trips --trip '<k=v;...>' [--detailed] | compare dates | compare true-cost\n\
  normalize flights --text '<...>' --url '<...>' | leave calc <start> <end> [country]\n\
  fetch-weather [--dest slug] | share-token | mark-plan-deleted <plan>\n\
  set-plan-name <name> [--dest <slug>] | set-active-destination <slug>\n\
\n\
DB\n\
  db status | db token-status | db schema [<table>] | db exec \"<SQL>\" | db migrate\n\
  db seed {{plans|destination-refs|ota-knowledge|test-plan}} | db sync {{destinations|events}}\n\
  db fetch holidays | db cleanup-deleted [--confirm] | db query-offers\n\
\n\
Plan resolution: --plan-id > $TRAVEL_PLAN_ID > --travel-date > active > upcoming > most-recent.\n\
Note: the dashboard renders Traditional Chinese by default — pair --zh on content edits, or update *_zh via set-tod-zh / set-day-theme --zh, or the change won't show."
    );
}
