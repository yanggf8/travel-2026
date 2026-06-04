import type { CommandHandler, CliContext } from '../shared/types';
import { registerCommand } from './registry';

const HELP = `
Travel Update CLI - Quick updates to travel plan

Usage:
  npx ts-node src/cli/travel-update.ts <command> [options]

Commands:
  set-dates <start> <end> [reason]
    Set travel dates. Triggers cascade to invalidate dependent processes.
    Example: set-dates 2026-02-13 2026-02-17 "Agent offered Feb 13"

  scrape-package <url> [--pax N] [--dest slug]
    Scrape a package itinerary URL and import it into P3+4 offers.
    Example: scrape-package "https://www.besttour.com.tw/itinerary/TYO05MM260211AM" --pax 2

  update-offer <offer-id> <date> <availability> [price] [seats] [source]
    Update offer availability for a specific date.
    availability: available | sold_out | limited
    Example: update-offer besttour_TYO05MM260211AM 2026-02-13 available 27888 2 agent

  select-offer <offer-id> <date> [--no-populate]
    Select an offer for booking. Populates P3/P4 from offer by default.
    Example: select-offer besttour_TYO05MM260211AM 2026-02-13

  scaffold-itinerary [--dest slug] [--force]
    Create day skeletons for P5 itinerary based on date anchor.
    Generates arrival/full/departure day structures with flight transit notes.
    Use --force to overwrite existing itinerary.
    Example: scaffold-itinerary

  populate-itinerary --goals "<cluster1,cluster2,...>" [--pace relaxed|balanced|packed] [--assign "<cluster:day,...>"] [--dest slug] [--force]
    Populate itinerary sessions by adding activities from destination clusters (incremental; does not overwrite days).
    Example: populate-itinerary --goals "chanel_shopping,omiyage_premium,teamlab_roppongi,asakusa_classic" --pace balanced

  mark-booked [--dest slug]
    Mark package, flight, and hotel as booked (selected/populated → booking → booked).
    Use after user confirms booking is complete.
    Example: mark-booked

  set-airport-transfer <arrival|departure> <planned|booked> --selected "<title|route|duration_min?|price_yen?|schedule?>" [--candidate "<...>"]...
    Set airport transfer plan (selected + candidates) for arrival/departure.
    Spec fields are pipe-delimited. Only title and route are required.
    Example: set-airport-transfer arrival planned --selected "Limousine Bus|NRT T1 → Shiodome (Takeshiba)|85|3200|19:40 → ~21:05"

  set-flight <outbound|return> [--dest slug] [--flight SL396] [--airline "Thai Lion Air"] [--airline-code SL]
    [--from TPE] [--dep-terminal T1] [--dep 09:00] [--to KIX] [--arr-terminal T1] [--arr 12:30]
    [--date 2026-02-24] [--booked-date 2026-02-24]
    Manually set/update a flight leg. Use when booking separately (bypasses cascade populate).
    Example: set-flight outbound --dest kyoto_2026 --flight SL396 --airline "Thai Lion Air" --from TPE --dep 09:00 --to KIX --arr 12:30

  set-hotel [--dest slug] [--name "Hotel Name"] [--check-in YYYY-MM-DD] [--access "route"] [--note "notes"]
    Manually set/update hotel info. Pipe-delimit multiple access directions.
    Example: set-hotel --dest kyoto_2026 --name "APA Hotel Kyoto Ekimae" --check-in 2026-02-24 --access "JR Kyoto Station 3min"

  set-activity-booking <day> <session> <activity> <status> [--ref <ref>] [--book-by <date>]
    Set booking status for an activity.
    day: Day number (1-indexed)
    session: morning | noon | afternoon | evening
    activity: Activity ID or title (case-insensitive)
    status: not_required | pending | booked | waitlist
    Example: set-activity-booking 3 morning "teamLab Borderless" booked --ref "TLB-12345"
    Example: set-activity-booking 3 morning teamlab pending --book-by 2026-02-01

  set-activity-time <day> <session> <activity> [--start HH:MM] [--end HH:MM] [--fixed true|false]
    Set optional time fields for an activity (start/end/fixed).
    Example: set-activity-time 5 afternoon "Hotel checkout" --start 11:00 --fixed true

  set-route-segment <day> <sort_order> <from> <to> <mode> [--duration <min>] [--notes "<text>"] [--start-time HH:MM] [--plan-id <id>]
    Upsert a route segment on a day (0-based sort_order). mode: transit | walking | driving
    Example: set-route-segment 1 0 "關渡" "嘟嘟房桃園機場貨運1站" driving --duration 45 --start-time 06:00
    Example: set-route-segment 1 1 "嘟嘟房桃園機場貨運1站" "桃園國際機場第一航廈" transit --duration 15 --start-time 06:45

  set-route-segments-bulk <day> --json '[{"from":"A","to":"B","mode":"walking","duration":5},...]' [--dest slug] [--plan-id <id>]
    Replace ALL route segments for a day with the provided JSON array. Auto-assigns sort_order.
    Example: set-route-segments-bulk 4 --json '[{"from":"hotel","to":"京都駅","mode":"walking","duration":3},{"from":"京都駅","to":"稲荷駅","mode":"transit","duration":5,"notes":"JR奈良線"}]'

  set-day-theme <day> [theme] [--zh "<zh_title>"] [--dest slug] [--plan-id <id>]
    Set day theme/title. Use --zh to set Traditional Chinese title (shown by default).
    Example: set-day-theme 1 --zh "抵達京都・安頓"
    Example: set-day-theme 2 "Kinkaku-ji day" --zh "金閣寺・伏見稲荷"

  set-activity-title <day> <session> <activity> "<new_title>" [--plan-id <id>]
    Rename an activity by title substring or ID.
    Example: set-activity-title 2 morning "Fushimi Inari" "金閣寺 (Kinkaku-ji)"

  delete-activity <day> <session> <activity_id_or_title> [--plan-id <id>]
    Remove an activity from a session. activity: ID or title substring.
    Aliases: remove-activity
    Example: delete-activity 2 morning "teamLab Borderless"

  set-tod-focus <day> <session> "<focus_text>" [--plan-id <id>]
    Set EN session focus summary.
    Aliases: set-session-focus
    Example: set-tod-focus 2 morning "Kitano Tenmangu → Kinkaku-ji"

  set-tod-time-range <day> <session> --start HH:MM --end HH:MM
    Set optional time boundaries for a session.
    Aliases: set-session-time-range
    Example: set-tod-time-range 5 afternoon --start 11:00 --end 14:45

  swap-days <dayA> <dayB> [--dest slug]
    Swap all activities between two days (preserves sessions).
    Useful for reordering itinerary without manual re-assignment.
    Example: swap-days 2 3

  validate-itinerary [--dest slug] [--severity error|warning|info] [--json]
    Validate itinerary for time conflicts, business hours, booking deadlines, and area efficiency.
    Example: validate-itinerary --severity warning

  fetch-weather [--dest slug] [--all]
    Fetch weather forecast from Open-Meteo and store on itinerary days.
    Requires itinerary to be scaffolded first. Dates must be within 16-day forecast window.
    Use --all to fetch weather for every destination that has itinerary days.
    Example: fetch-weather
    Example: fetch-weather --all

  search-offers --dest slug [--start YYYY-MM-DD] [--end YYYY-MM-DD] [--pax N] [--types package,flight,hotel] [--source id] [--json]
    Search across registered OTA scrapers (if any are registered at runtime).
    If --start/--end are omitted, uses the destination confirmed dates.
    Example: search-offers --dest tokyo_2026 --pax 2 --types package --json

  compare-offers --region <name> [--date YYYY-MM-DD] [--pax N] [--json]
    Compare imported Turso offers by region.
    Reads the Turso offers table (no new scraping).
    region: osaka, kansai, tokyo, etc.
    Example: compare-offers --region osaka --date 2026-02-26 --pax 2

  view-prices --start YYYY-MM-DD --end YYYY-MM-DD [--region name] [--destination slug] [--hotel-per-night TWD] [--nights N] [--package TWD] [--pax N] [--json]
    Compare package vs separate booking (flight+hotel) across departure dates.
    Reads imported Turso flight/hotel offers.
    --hotel-per-night: Hotel cost per night in TWD (default: auto-detect from Turso hotel offers)
    --nights: Number of hotel nights
    --package: Package price for all pax in TWD (for comparison column)
    Example: view-prices --start 2026-02-24 --end 2026-02-28 --region kansai --hotel-per-night 3000 --nights 4 --package 40740

  query-offers [--region name] [--start YYYY-MM-DD] [--end YYYY-MM-DD] [--sources csv] [--max-price N] [--fresh-hours N] [--max N] [--json]
    Query offers from Turso cloud database with filters.
    Example: query-offers --region kansai --start 2026-02-24 --end 2026-02-28
    Note: --start/--end are offer filters, not plan selectors.

  add-offer --run <run_id> --source <id> --region <code> --depart <YYYY-MM-DD> --return <YYYY-MM-DD> --nights N --price <twd> --title "<title>" --url <url> [--hotel "<name>"] [--kind fit|group_tour] [--seats N] [--note "<text>"]
    Directly insert a single manual FIT or group-tour offer into Turso DB (no JSON file required).
    Designed for quickly recording data while browsing product pages during Shaping Stage research.

  add-besttour-offer --url <besttour-itinerary-url> --price <twd> --hotel "<name>" [--seats N] [--note "..."] [--run <run_id>]
    Fast BestTour-specific version of add-offer.
    Auto-infers nights and dates from the product code in the URL (e.g. OKA04FD260612BU).
    Example: add-besttour-offer --url "https://www.besttour.com.tw/itinerary/OKA04FD260612BU" --price 16888 --hotel "WBF水之都那霸酒店" --seats 2 --note "FD230 13:30 | 只有6/12跟6/26"

  add-lifetour-offer --url <lifetour url> --price <twd> --hotel "<name>" [--depart YYYY-MM-DD] [--return YYYY-MM-DD] [--seats N] [--note "..."] [--run <run_id>]
    Fast helper for 五福旅遊 (Lifetour). Works with both search list pages and product pages.
    Example: add-lifetour-offer --url "https://tour.lifetour.com.tw/searchlist/tpe/0001-0005" --price 17200 --hotel "水之都那霸飯店" --seats 2 --depart 2026-06-21 --return 2026-06-24 --note "沖繩機加酒"

  chat-format --run <run_id> [--region okinawa] [--max N] [--hotel "substring"]
    Print qualified candidates in clean messenger/chat format (ready to copy-paste).
    Use --hotel to filter (e.g. "水之都" or "Mercure").
    Example: chat-format --run shaping-20260525-093508 --region okinawa --hotel "水之都"

  check-freshness --source <id> [--region name] [--start YYYY-MM-DD] [--end YYYY-MM-DD] [--max-age N]
    Check if Turso has fresh data for a source/region. Returns skip/rescrape/no_data.
    Example: check-freshness --source besttour --region kansai

  sync-bookings [--plan path] [--state path] [--trip-id id] [--dry-run]
    Extract bookings from travel-plan.json and sync to Turso. Idempotent.
    Example: sync-bookings

  query-bookings [--dest slug] [--category package|transfer|activity] [--status pending|booked] [--trip-id id] [--json]
    Query bookings from Turso DB.
    Example: query-bookings --dest tokyo_2026 --status pending

  check-booking-integrity [--trip-id id]
    Compare bookings in plan JSON vs Turso DB.
    Example: check-booking-integrity

  run-status [run-id]
    Show details for a specific operation run, or the most recent run if no ID given.
    Example: run-status
    Example: run-status a1b2c3d4-...

  run-list [--status completed|failed|started] [--limit N]
    List recent operation runs for this plan.
    Example: run-list
    Example: run-list --status failed --limit 5

  shaping-init --origin <IATA> --start <date> --end <date> --dest CODE:LABEL --nights N [--shaping ...]
    Create a Shaping Stage triangle-research run (immutable inputs; pre-plan).
    Repeat --dest and --nights for multiple destinations/durations.
    Use --shaping ASPECT:ROLE:KIND:VALUE[:NOTES] (repeatable) for hard constraints, preferences, etc.
    Example: shaping-init --origin TPE --start 2026-06-18 --end 2026-06-20 --dest KIX:"Osaka (KIX)" --nights 6
    Example: ... --shaping date:hard_constraint:return_no_later_than:2026-06-27 --shaping channel:hard_constraint:exclude_source:kkday

  shaping-export --run <run_id> --json
    Export a Shaping Stage run as JSON (consumed by the aggregator script).

  shaping-import --run <run_id> --file <path>
    Import Shaping Stage aggregator results from a handoff JSON file.

  shaping-compare --run <run_id> [--json] [--limit N]
    Show ranked Shaping Stage flight candidates across destinations.
    Example: shaping-compare --run shaping-20260522-143000

  shaping-adopt <candidate_id> <plan_id> [--create-plan --dest <slug>]
    Record a Shaping Stage candidate as adopted into a plan.
    With --create-plan, seed a new plan with P1 dates and P2 destination from the candidate.

  plans [--json]
    List DB travel plans with active destination and date anchors.
    Alias: list-plans

  status
    Show current plan status summary.

  itinerary [--dest slug]
    Show daily itinerary with transport details.

  transport [--dest slug]
    Show transport summary (airport + daily transit).

  bookings [--dest slug]
    Show pending bookings only.

  help
    Show this help message.

Options:
  --plan-id <id> Plan ID (preferred, e.g., 'kyoto-2026' or $TRAVEL_PLAN_ID)
  --plan <path>  Travel plan path (deprecated, use --plan-id instead)
  --travel-date <date> Resolve plan by itinerary date (YYYY-MM-DD)
  --travel-start <date> --travel-end <date>
                Resolve plan by confirmed date anchor or candidate planning-window overlap
  --state <path> State log path (or $TRAVEL_STATE_PATH)
  --dry-run    Show what would be changed without saving
  --verbose    Show detailed output
  --full       Show booked offer/flight/hotel details (status only)
  --force      Allow overwrites / bypass safeguards (command-specific)
`;

export { HELP };

const helpCommand: CommandHandler = {
  names: ['help'],
  description: 'Show help message',
  usage: 'help',
  async execute() {
    console.log(HELP);
  },
};

registerCommand(helpCommand);
