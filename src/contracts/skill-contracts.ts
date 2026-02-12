/**
 * Skill Contracts v1.9.0
 *
 * Defines the interface for all CLI operations.
 * Agent can query this to discover available operations.
 *
 * Versioning: semver
 * - MAJOR: breaking changes to args/output shape
 * - MINOR: new operations or optional args
 * - PATCH: bug fixes, no interface change
 *
 * v1.9.0 - Added operation tracking (run-status, run-list, saveWithTracking, monotonic version counter)
 * v1.8.0 - Added weather forecast fetch (fetch-weather)
 * v1.7.0 - DB-primary migration: writePlanToDb/readPlanFromDb, async save(), StateManager.create()
 * v1.6.0 - Added booking sync/query operations (sync-bookings, query-bookings, snapshot-plan, check-booking-integrity)
 * v1.5.0 - Added Turso DB operations (query-offers, check-freshness, import-offers)
 * v1.4.0 - Added data_freshness tier to SkillContract for staleness awareness
 * v1.3.0 - Added view operations (status, itinerary, transport, bookings)
 * v1.2.0 - Added itinerary validation, scraper registry, and project init APIs
 * v1.1.0 - Added configuration discovery APIs and multi-destination support
 */

export const CONTRACT_VERSION = '1.9.0';

/**
 * Data freshness tiers.
 *
 * Tells the agent what kind of data an operation produces or consumes,
 * so it can decide whether to re-scrape or trust cached results.
 *
 * - live:   Real-time fetch (scraper / API call). Always current.
 * - cached: Reads from previously-scraped files (scrapes/*.json).
 *           May be stale — agent should check scraped_at timestamp.
 * - static: Plan state, config, or reference data. Doesn't go stale
 *           (changes only when the user mutates it).
 */
export type DataFreshness = 'live' | 'cached' | 'static';

export interface SkillContract {
  name: string;
  description: string;
  args: {
    name: string;
    type: 'string' | 'number' | 'boolean' | 'string[]';
    required: boolean;
    description: string;
  }[];
  output: {
    type: 'void' | 'object' | 'array' | 'string';
    description: string;
  };
  mutates: string[];  // State keys this operation may change
  data_freshness: DataFreshness;  // What tier of data this operation works with
  example: string;
}

/**
 * All available CLI operations with their contracts.
 * Agent should query this before invoking operations.
 */
export const SKILL_CONTRACTS: Record<string, SkillContract> = {
  'set-dates': {
    name: 'set-dates',
    description: 'Set travel dates. Triggers cascade to invalidate dependent processes.',
    args: [
      { name: 'start', type: 'string', required: true, description: 'Start date (YYYY-MM-DD)' },
      { name: 'end', type: 'string', required: true, description: 'End date (YYYY-MM-DD)' },
      { name: 'reason', type: 'string', required: false, description: 'Reason for change' },
    ],
    output: { type: 'void', description: 'Updates state files' },
    mutates: [
      'travel-plan.destinations.*.process_1_date_anchor',
      'travel-plan.cascade_state',
      'state.event_log',
    ],
    data_freshness: 'static',
    example: 'npm run travel -- set-dates 2026-02-13 2026-02-17 "Agent offered Feb 13"',
  },

  'select-offer': {
    name: 'select-offer',
    description: 'Select a package offer for booking. Populates P3/P4 from offer.',
    args: [
      { name: 'offer-id', type: 'string', required: true, description: 'Offer ID (e.g., besttour_TYO05MM260211AM)' },
      { name: 'date', type: 'string', required: true, description: 'Booking date (YYYY-MM-DD)' },
      { name: '--no-populate', type: 'boolean', required: false, description: 'Skip populating P3/P4' },
    ],
    output: { type: 'void', description: 'Updates state files' },
    mutates: [
      'travel-plan.destinations.*.process_3_4_packages',
      'travel-plan.destinations.*.process_3_transportation',
      'travel-plan.destinations.*.process_4_accommodation',
      'state.event_log',
    ],
    data_freshness: 'cached',
    example: 'npm run travel -- select-offer besttour_TYO05MM260211AM 2026-02-13',
  },

  'mark-booked': {
    name: 'mark-booked',
    description: 'Mark package, flight, and hotel as booked after user confirms.',
    args: [
      { name: '--dest', type: 'string', required: false, description: 'Destination slug (default: active)' },
    ],
    output: { type: 'void', description: 'Updates state files' },
    mutates: [
      'travel-plan.destinations.*.process_3_4_packages.status',
      'travel-plan.destinations.*.process_3_transportation.status',
      'travel-plan.destinations.*.process_4_accommodation.status',
      'state.next_actions',
      'state.current_focus',
      'state.event_log',
    ],
    data_freshness: 'static',
    example: 'npm run travel -- mark-booked',
  },

  'scaffold-itinerary': {
    name: 'scaffold-itinerary',
    description: 'Create day skeletons for P5 itinerary based on date anchor.',
    args: [
      { name: '--dest', type: 'string', required: false, description: 'Destination slug (default: active)' },
      { name: '--force', type: 'boolean', required: false, description: 'Overwrite existing itinerary' },
    ],
    output: { type: 'void', description: 'Updates state files' },
    mutates: [
      'travel-plan.destinations.*.process_5_daily_itinerary',
      'state.event_log',
    ],
    data_freshness: 'static',
    example: 'npm run travel -- scaffold-itinerary --force',
  },

  'populate-itinerary': {
    name: 'populate-itinerary',
    description: 'Populate itinerary sessions with activities from clusters.',
    args: [
      { name: '--goals', type: 'string', required: true, description: 'Comma-separated cluster names' },
      { name: '--pace', type: 'string', required: false, description: 'relaxed|balanced|packed (default: balanced)' },
      { name: '--assign', type: 'string', required: false, description: 'Manual cluster:day assignments' },
      { name: '--dest', type: 'string', required: false, description: 'Destination slug' },
      { name: '--force', type: 'boolean', required: false, description: 'Overwrite existing activities' },
    ],
    output: { type: 'void', description: 'Updates state files' },
    mutates: [
      'travel-plan.destinations.*.process_5_daily_itinerary.days',
      'state.event_log',
    ],
    data_freshness: 'static',
    example: 'npm run travel -- populate-itinerary --goals "chanel_shopping,teamlab_roppongi" --pace balanced',
  },

  'status': {
    name: 'status',
    description: 'Show current plan status summary. Read-only.',
    args: [
      { name: '--full', type: 'boolean', required: false, description: 'Show flight/hotel details' },
    ],
    output: { type: 'string', description: 'Formatted status output' },
    mutates: [],  // Read-only
    data_freshness: 'static',
    example: 'npm run travel -- status',
  },

  'set-airport-transfer': {
    name: 'set-airport-transfer',
    description: 'Set airport transfer plan (selected + candidates) for arrival/departure.',
    args: [
      { name: 'direction', type: 'string', required: true, description: 'arrival | departure' },
      { name: 'status', type: 'string', required: true, description: 'planned | booked' },
      { name: '--selected', type: 'string', required: true, description: 'Pipe-delimited spec: title|route|duration_min?|price_yen?|schedule?' },
      { name: '--candidate', type: 'string', required: false, description: 'Repeatable pipe-delimited spec for candidates' },
      { name: '--dest', type: 'string', required: false, description: 'Destination slug (default: active)' },
    ],
    output: { type: 'void', description: 'Updates state files' },
    mutates: [
      'travel-plan.destinations.*.process_3_transportation.airport_transfers',
      'state.event_log',
    ],
    data_freshness: 'static',
    example: 'npm run travel -- set-airport-transfer arrival planned --selected "Limousine Bus|NRT T2 → Shiodome (Takeshiba)|85|3200|19:40 → ~21:05"',
  },

  'set-activity-booking': {
    name: 'set-activity-booking',
    description: 'Set booking status for an activity (tracks confirmation state).',
    args: [
      { name: 'day', type: 'number', required: true, description: 'Day number (1-indexed)' },
      { name: 'session', type: 'string', required: true, description: 'morning | afternoon | evening' },
      { name: 'activity', type: 'string', required: true, description: 'Activity ID or title (case-insensitive)' },
      { name: 'status', type: 'string', required: true, description: 'not_required | pending | booked | waitlist' },
      { name: '--ref', type: 'string', required: false, description: 'Booking reference/confirmation number' },
      { name: '--book-by', type: 'string', required: false, description: 'Deadline to book (YYYY-MM-DD)' },
      { name: '--dest', type: 'string', required: false, description: 'Destination slug (default: active)' },
    ],
    output: { type: 'void', description: 'Updates state files' },
    mutates: [
      'travel-plan.destinations.*.process_5_daily_itinerary.days.*.{session}.activities.*.booking_status',
      'travel-plan.destinations.*.process_5_daily_itinerary.days.*.{session}.activities.*.booking_ref',
      'travel-plan.destinations.*.process_5_daily_itinerary.days.*.{session}.activities.*.book_by',
      'state.event_log',
    ],
    data_freshness: 'static',
    example: 'npm run travel -- set-activity-booking 3 morning "teamLab Borderless" booked --ref "TLB-12345"',
  },

  'swap-days': {
    name: 'swap-days',
    description: 'Swap all activities between two days (preserves sessions and day metadata).',
    args: [
      { name: 'dayA', type: 'number', required: true, description: 'First day number (1-indexed)' },
      { name: 'dayB', type: 'number', required: true, description: 'Second day number (1-indexed)' },
      { name: '--dest', type: 'string', required: false, description: 'Destination slug (default: active)' },
    ],
    output: { type: 'void', description: 'Updates state files' },
    mutates: [
      'travel-plan.destinations.*.process_5_daily_itinerary.days.*.morning',
      'travel-plan.destinations.*.process_5_daily_itinerary.days.*.afternoon',
      'travel-plan.destinations.*.process_5_daily_itinerary.days.*.evening',
      'travel-plan.destinations.*.process_5_daily_itinerary.days.*.theme',
      'travel-plan.destinations.*.process_5_daily_itinerary.days.*.notes',
      'state.event_log',
    ],
    data_freshness: 'static',
    example: 'npm run travel -- swap-days 2 3',
  },

  'set-activity-time': {
    name: 'set-activity-time',
    description: 'Set optional time fields for an activity (start/end/fixed).',
    args: [
      { name: 'day', type: 'number', required: true, description: 'Day number (1-indexed)' },
      { name: 'session', type: 'string', required: true, description: 'morning | afternoon | evening' },
      { name: 'activity', type: 'string', required: true, description: 'Activity ID or title (case-insensitive)' },
      { name: '--start', type: 'string', required: false, description: 'Start time (HH:MM)' },
      { name: '--end', type: 'string', required: false, description: 'End time (HH:MM)' },
      { name: '--fixed', type: 'string', required: false, description: 'true|false (hard constraint)' },
      { name: '--dest', type: 'string', required: false, description: 'Destination slug (default: active)' },
    ],
    output: { type: 'void', description: 'Updates state files' },
    mutates: [
      'travel-plan.destinations.*.process_5_daily_itinerary.days.*.{session}.activities.*.start_time',
      'travel-plan.destinations.*.process_5_daily_itinerary.days.*.{session}.activities.*.end_time',
      'travel-plan.destinations.*.process_5_daily_itinerary.days.*.{session}.activities.*.is_fixed_time',
      'state.event_log',
    ],
    data_freshness: 'static',
    example: 'npm run travel -- set-activity-time 5 afternoon "Hotel checkout" --start 11:00 --fixed true',
  },

  'set-session-time-range': {
    name: 'set-session-time-range',
    description: 'Set optional time boundaries for a session.',
    args: [
      { name: 'day', type: 'number', required: true, description: 'Day number (1-indexed)' },
      { name: 'session', type: 'string', required: true, description: 'morning | afternoon | evening' },
      { name: '--start', type: 'string', required: true, description: 'Session start time (HH:MM)' },
      { name: '--end', type: 'string', required: true, description: 'Session end time (HH:MM)' },
      { name: '--dest', type: 'string', required: false, description: 'Destination slug (default: active)' },
    ],
    output: { type: 'void', description: 'Updates state files' },
    mutates: [
      'travel-plan.destinations.*.process_5_daily_itinerary.days.*.{session}.time_range',
      'state.event_log',
    ],
    data_freshness: 'static',
    example: 'npm run travel -- set-session-time-range 5 afternoon --start 11:00 --end 14:45',
  },

  'update-offer': {
    name: 'update-offer',
    description: 'Update offer availability for a specific date.',
    args: [
      { name: 'offer-id', type: 'string', required: true, description: 'Offer ID' },
      { name: 'date', type: 'string', required: true, description: 'Date (YYYY-MM-DD)' },
      { name: 'availability', type: 'string', required: true, description: 'available|sold_out|limited' },
      { name: 'price', type: 'number', required: false, description: 'New price' },
      { name: 'seats', type: 'number', required: false, description: 'Seats remaining' },
      { name: 'source', type: 'string', required: false, description: 'Info source (agent|scrape|user)' },
    ],
    output: { type: 'void', description: 'Updates state files' },
    mutates: [
      'travel-plan.destinations.*.process_3_4_packages.results.offers',
      'state.event_log',
    ],
    data_freshness: 'cached',
    example: 'npm run travel -- update-offer besttour_TYO05MM260211AM 2026-02-13 available 27888 2 agent',
  },

  'validate-itinerary': {
    name: 'validate-itinerary',
    description: 'Validate itinerary for time conflicts, business hours, booking deadlines, and area efficiency.',
    args: [
      { name: '--dest', type: 'string', required: false, description: 'Destination slug (default: active)' },
      { name: '--severity', type: 'string', required: false, description: 'Minimum severity to show (error|warning|info, default: info)' },
      { name: '--json', type: 'boolean', required: false, description: 'Output as JSON' },
    ],
    output: { type: 'object', description: 'Validation result with issues array' },
    mutates: [],  // Read-only
    data_freshness: 'static',
    example: 'npm run travel -- validate-itinerary --severity warning',
  },

  'init-project': {
    name: 'init-project',
    description: 'Initialize a new travel plan with proper structure.',
    args: [
      { name: '--dest', type: 'string', required: true, description: 'Destination slug (e.g., tokyo_2026)' },
      { name: '--start', type: 'string', required: true, description: 'Start date (YYYY-MM-DD)' },
      { name: '--end', type: 'string', required: true, description: 'End date (YYYY-MM-DD)' },
      { name: '--pax', type: 'number', required: false, description: 'Number of travelers (default: 2)' },
      { name: '--output', type: 'string', required: false, description: 'Output directory (default: data)' },
    ],
    output: { type: 'object', description: '{ planPath, statePath }' },
    mutates: ['travel-plan.json', 'state.json'],
    data_freshness: 'static',
    example: 'npx ts-node src/templates/project-init.ts --dest osaka_2026 --start 2026-04-01 --end 2026-04-05',
  },

  'search-offers': {
    name: 'search-offers',
    description: 'Search for offers across all registered OTA scrapers.',
    args: [
      { name: '--dest', type: 'string', required: true, description: 'Destination slug' },
      { name: '--start', type: 'string', required: false, description: 'Start date (YYYY-MM-DD). Defaults to confirmed dates in plan.' },
      { name: '--end', type: 'string', required: false, description: 'End date (YYYY-MM-DD). Defaults to confirmed dates in plan.' },
      { name: '--pax', type: 'number', required: false, description: 'Number of travelers (default: 2)' },
      { name: '--types', type: 'string', required: false, description: 'Comma-separated product types (package,flight,hotel)' },
      { name: '--source', type: 'string', required: false, description: 'Specific OTA source ID' },
    ],
    output: { type: 'array', description: 'Array of ScrapeResult objects' },
    mutates: [],  // Read-only (use import-offers to write)
    data_freshness: 'live',
    example: 'npm run travel -- search-offers --dest tokyo_2026 --start 2026-02-13 --end 2026-02-17',
  },

  // === View Operations (read-only) ===

  'view-status': {
    name: 'view-status',
    description: 'Booking overview with fixed-time activities and deadlines.',
    args: [
      { name: '--dest', type: 'string', required: false, description: 'Destination slug (default: active)' },
    ],
    output: { type: 'string', description: 'Formatted status overview' },
    mutates: [],
    data_freshness: 'static',
    example: 'npm run view:status',
  },

  'view-itinerary': {
    name: 'view-itinerary',
    description: 'Daily plan with activities, meals, and transport notes per session.',
    args: [
      { name: '--dest', type: 'string', required: false, description: 'Destination slug (default: active)' },
      { name: '--day', type: 'number', required: false, description: 'Show specific day only' },
    ],
    output: { type: 'string', description: 'Formatted daily itinerary' },
    mutates: [],
    data_freshness: 'static',
    example: 'npm run view:itinerary',
  },

  'view-transport': {
    name: 'view-transport',
    description: 'Transport summary including airport transfers and daily transit.',
    args: [
      { name: '--dest', type: 'string', required: false, description: 'Destination slug (default: active)' },
    ],
    output: { type: 'string', description: 'Formatted transport summary' },
    mutates: [],
    data_freshness: 'static',
    example: 'npm run view:transport',
  },

  'view-bookings': {
    name: 'view-bookings',
    description: 'Pending and confirmed bookings with deadlines and references.',
    args: [
      { name: '--dest', type: 'string', required: false, description: 'Destination slug (default: active)' },
      { name: '--status', type: 'string', required: false, description: 'Filter by status (pending|booked|all, default: all)' },
    ],
    output: { type: 'string', description: 'Formatted bookings list' },
    mutates: [],
    data_freshness: 'static',
    example: 'npm run view:bookings',
  },

  // === Turso DB Operations ===

  'query-offers': {
    name: 'query-offers',
    description: 'Query offers from Turso cloud database with filters.',
    args: [
      { name: '--region', type: 'string', required: false, description: 'Region (kansai, tokyo)' },
      { name: '--start', type: 'string', required: false, description: 'Departure date >=' },
      { name: '--end', type: 'string', required: false, description: 'Departure date <=' },
      { name: '--sources', type: 'string', required: false, description: 'OTA source IDs (csv)' },
      { name: '--max-price', type: 'number', required: false, description: 'Max price per person' },
      { name: '--fresh-hours', type: 'number', required: false, description: 'Only offers scraped within N hours' },
      { name: '--max', type: 'number', required: false, description: 'Max results to return' },
      { name: '--json', type: 'boolean', required: false, description: 'JSON output' },
    ],
    output: { type: 'array', description: 'Offer records from Turso' },
    mutates: [],
    data_freshness: 'cached',
    example: 'npm run travel -- query-offers --region kansai --start 2026-02-24 --end 2026-02-28',
  },

  'check-freshness': {
    name: 'check-freshness',
    description: 'Check if Turso has fresh data for a source/region. Returns skip/rescrape/no_data.',
    args: [
      { name: '--source', type: 'string', required: true, description: 'OTA source ID' },
      { name: '--region', type: 'string', required: false, description: 'Region filter' },
      { name: '--max-age', type: 'number', required: false, description: 'Max age hours (default: 24)' },
    ],
    output: { type: 'object', description: '{ hasFreshData, ageHours, offerCount, recommendation }' },
    mutates: [],
    data_freshness: 'cached',
    example: 'npm run travel -- check-freshness --source besttour --region kansai',
  },

  'import-offers': {
    name: 'import-offers',
    description: 'Import scraped files into Turso. Auto-triggered by scrape-package.',
    args: [
      { name: '--dir', type: 'string', required: false, description: 'Directory (default: scrapes)' },
      { name: '--files', type: 'string', required: false, description: 'Comma-separated file paths' },
    ],
    output: { type: 'object', description: '{ imported, skipped, filtered }' },
    mutates: ['turso.offers'],
    data_freshness: 'live',
    example: 'npm run db:import:turso -- --dir scrapes',
  },

  // === Booking Operations ===

  'sync-bookings': {
    name: 'sync-bookings',
    description: 'Extract bookings from travel-plan.json and sync to Turso bookings_current. Idempotent.',
    args: [
      { name: '--plan', type: 'string', required: false, description: 'Plan file path' },
      { name: '--state', type: 'string', required: false, description: 'State file path' },
      { name: '--trip-id', type: 'string', required: false, description: 'Trip ID (default: inferred from path)' },
      { name: '--dry-run', type: 'boolean', required: false, description: 'Show what would sync without writing' },
    ],
    output: { type: 'object', description: '{ synced: number, warnings: string[] }' },
    mutates: ['turso.bookings_current', 'turso.bookings_events'],
    data_freshness: 'static',
    example: 'npm run travel -- sync-bookings',
  },

  'query-bookings': {
    name: 'query-bookings',
    description: 'Query bookings from Turso DB with filters.',
    args: [
      { name: '--dest', type: 'string', required: false, description: 'Destination slug' },
      { name: '--category', type: 'string', required: false, description: 'package|transfer|activity' },
      { name: '--status', type: 'string', required: false, description: 'Booking status filter' },
      { name: '--trip-id', type: 'string', required: false, description: 'Trip ID filter' },
      { name: '--json', type: 'boolean', required: false, description: 'JSON output' },
    ],
    output: { type: 'array', description: 'BookingCurrentRow[] from Turso' },
    mutates: [],
    data_freshness: 'cached',
    example: 'npm run travel -- query-bookings --dest tokyo_2026 --status pending',
  },

  'snapshot-plan': {
    name: 'snapshot-plan',
    description: 'Archive current plan+state to Turso plan_snapshots.',
    args: [
      { name: '--trip-id', type: 'string', required: false, description: 'Trip ID (default: japan-2026)' },
    ],
    output: { type: 'object', description: '{ snapshot_id: string, trip_id: string }' },
    mutates: ['turso.plan_snapshots'],
    data_freshness: 'static',
    example: 'npm run travel -- snapshot-plan --trip-id japan-2026',
  },

  'check-booking-integrity': {
    name: 'check-booking-integrity',
    description: 'Compare bookings in plan JSON vs Turso DB. Reports matches, mismatches, plan-only, DB-only.',
    args: [
      { name: '--trip-id', type: 'string', required: false, description: 'Trip ID filter' },
    ],
    output: { type: 'object', description: '{ matches, mismatches, dbOnly, planOnly }' },
    mutates: [],
    data_freshness: 'cached',
    example: 'npm run travel -- check-booking-integrity',
  },

  'fetch-weather': {
    name: 'fetch-weather',
    description: 'Fetch weather forecast from Open-Meteo and store on itinerary days.',
    args: [
      { name: '--dest', type: 'string', required: false, description: 'Destination slug (default: active)' },
    ],
    output: { type: 'void', description: 'Updates weather field on itinerary days' },
    mutates: [
      'travel-plan.destinations.*.process_5_daily_itinerary.days.*.weather',
      'state.event_log',
    ],
    data_freshness: 'live',
    example: 'npm run travel -- fetch-weather',
  },

  // === Operation Tracking (v1.9.0) ===

  'run-status': {
    name: 'run-status',
    description: 'Show operation run details. Without args, shows the most recent run.',
    args: [
      { name: 'run-id', type: 'string', required: false, description: 'Specific run ID (default: most recent)' },
    ],
    output: { type: 'object', description: 'Operation run details including status, version, timing' },
    mutates: [],
    data_freshness: 'live',
    example: 'npm run travel -- run-status',
  },

  'run-list': {
    name: 'run-list',
    description: 'List recent operations for the current plan.',
    args: [
      { name: '--status', type: 'string', required: false, description: 'Filter by status (started|completed|failed)' },
      { name: '--limit', type: 'number', required: false, description: 'Max results (default: 20)' },
    ],
    output: { type: 'array', description: 'Table of recent operation runs' },
    mutates: [],
    data_freshness: 'live',
    example: 'npm run travel -- run-list --status failed',
  },

};

/**
 * StateManager method contracts.
 * Documents all public methods for agent discovery.
 */
export const STATE_MANAGER_METHODS = {
  // Timestamp
  now: { returns: 'string', description: 'Get session timestamp' },
  freshTimestamp: { returns: 'string', description: 'Generate new timestamp' },
  refreshTimestamp: { returns: 'void', description: 'Refresh session timestamp' },

  // Dirty flags
  markDirty: { args: ['destination', 'process'], returns: 'void', description: 'Mark process dirty' },
  clearDirty: { args: ['destination', 'process'], returns: 'void', description: 'Clear dirty flag' },
  markGlobalDirty: { args: ['process'], returns: 'void', description: 'Mark global process dirty' },
  isDirty: { args: ['destination', 'process'], returns: 'boolean', description: 'Check if dirty' },

  // Process status
  setProcessStatus: { args: ['destination', 'process', 'status'], returns: 'void', description: 'Set process status' },
  getProcessStatus: { args: ['destination', 'process'], returns: 'ProcessStatus|null', description: 'Get process status' },

  // Events
  emitEvent: { args: ['event'], returns: 'void', description: 'Emit audit event' },
  getEventLog: { args: [], returns: 'TravelEvent[]', description: 'Get event log' },

  // Date anchor
  setDateAnchor: { args: ['startDate', 'endDate', 'reason?'], returns: 'void', description: 'Set travel dates' },
  getDateAnchor: { args: [], returns: '{ start, end, days }|null', description: 'Get travel dates' },

  // Offers
  updateOfferAvailability: { args: ['offerId', 'date', 'availability', 'price?', 'seats?', 'source?'], returns: 'void', description: 'Update offer availability' },
  selectOffer: { args: ['offerId', 'date', 'populateCascade?'], returns: 'void', description: 'Select offer for booking' },
  importPackageOffers: { args: ['destination', 'sourceId', 'offers', 'note?', 'warnings?'], returns: 'void', description: 'Import scraped offers' },

  // Itinerary
  scaffoldItinerary: { args: ['destination', 'days', 'force?'], returns: 'void', description: 'Create day skeletons' },
  addActivity: { args: ['destination', 'dayNumber', 'session', 'activity'], returns: 'string', description: 'Add activity, returns ID' },
  updateActivity: { args: ['destination', 'dayNumber', 'session', 'activityId', 'updates'], returns: 'void', description: 'Update activity' },
  removeActivity: { args: ['destination', 'dayNumber', 'session', 'activityId'], returns: 'void', description: 'Remove activity' },
  setDayTheme: { args: ['destination', 'dayNumber', 'theme'], returns: 'void', description: 'Set day theme' },
  setDayWeather: { args: ['destination', 'dayNumber', 'weather'], returns: 'void', description: 'Set day weather forecast' },
  setSessionFocus: { args: ['destination', 'dayNumber', 'session', 'focus'], returns: 'void', description: 'Set session focus' },
  setActivityBookingStatus: { args: ['destination', 'dayNumber', 'session', 'activityIdOrTitle', 'status', 'ref?', 'bookBy?'], returns: 'void', description: 'Set activity booking status' },
  setActivityTime: { args: ['destination', 'dayNumber', 'session', 'activityIdOrTitle', 'opts'], returns: 'void', description: 'Set activity time fields (start/end/fixed)' },
  setSessionTimeRange: { args: ['destination', 'dayNumber', 'session', 'start', 'end'], returns: 'void', description: 'Set session time range boundary' },
  findActivity: { args: ['destination', 'idOrTitle'], returns: '{ dayNumber, session, activity } | null', description: 'Find activity by ID or title' },

  // Airport transfers
  setAirportTransferSegment: { args: ['destination', 'direction', 'segment'], returns: 'void', description: 'Set airport transfer segment (arrival/departure)' },
  addAirportTransferCandidate: { args: ['destination', 'direction', 'option'], returns: 'void', description: 'Add candidate airport transfer option' },
  selectAirportTransferOption: { args: ['destination', 'direction', 'optionId'], returns: 'void', description: 'Select airport transfer option by ID' },

  // Destination
  getActiveDestination: { args: [], returns: 'string', description: 'Get active destination slug' },
  setActiveDestination: { args: ['destination'], returns: 'void', description: 'Set active destination' },
  setFocus: { args: ['destination', 'process'], returns: 'void', description: 'Set current focus' },
  setNextActions: { args: ['actions'], returns: 'void', description: 'Set next actions list' },
  getNextActions: { args: [], returns: 'string[]', description: 'Get next actions list' },

  // I/O
  save: { args: [], returns: 'Promise<void>', description: 'Save plan+state to DB (blocking), then sync derived tables' },
  saveWithTracking: { args: ['commandType', 'commandSummary?'], returns: 'Promise<{ run_id: string; version: number }>', description: 'Save with operation audit trail (version is monotonic counter, no lock)' },
  getPlan: { args: [], returns: 'TravelPlanMinimal', description: 'Get current plan object' },
  getPlanId: { args: [], returns: 'string', description: 'Get the plan ID for this state manager instance' },
} as const;

/**
 * Turso DB-primary service contracts (v1.7.0).
 */
export const TURSO_SERVICE_CONTRACTS = {
  derivePlanId: { args: ['planPath'], returns: 'string', description: 'Derive plan ID from file path' },
  writePlanToDb: { args: ['planId', 'planJson', 'stateJson', 'schemaVersion'], returns: 'Promise<void>', description: 'Upsert plan+state to plans' },
  readPlanFromDb: { args: ['planId'], returns: 'Promise<{plan_json, state_json, updated_at} | null>', description: 'Read plan+state from plans' },
  syncEventsToDb: { args: ['events'], returns: 'Promise<{synced, skipped}>', description: 'Idempotent event sync via SHA1 external_id' },
} as const;

/**
 * StateManager static factory (v1.7.0).
 */
export const STATE_MANAGER_FACTORY = {
  create: { args: ['planPathOrOpts?', 'statePath?'], returns: 'Promise<StateManager>', description: 'DB-only factory: load plan+state from Turso plans' },
  derivePlanId: { args: ['planPath'], returns: 'string', description: 'Derive plan ID from file path' },
} as const;

/**
 * Get contract for a skill by name.
 */
export function getSkillContract(name: string): SkillContract | undefined {
  return SKILL_CONTRACTS[name];
}

/**
 * List all available skills.
 */
export function listSkills(): string[] {
  return Object.keys(SKILL_CONTRACTS);
}

/**
 * Validate that StateManager has all documented methods.
 * Call this at startup to catch interface drift.
 */
export function validateStateManagerInterface(sm: object): string[] {
  const missing: string[] = [];
  for (const method of Object.keys(STATE_MANAGER_METHODS)) {
    if (typeof (sm as Record<string, unknown>)[method] !== 'function') {
      missing.push(method);
    }
  }
  return missing;
}

/**
 * Configuration Discovery APIs (src/config/loader.ts)
 * These APIs enable multi-destination and multi-OTA support.
 */
export const CONFIG_LOADER_APIS = {
  // Destination Discovery
  getAvailableDestinations: {
    args: [],
    returns: 'string[]',
    description: 'List all configured destination slugs',
  },
  getDestinationConfig: {
    args: ['slug'],
    returns: 'DestinationConfig | null',
    description: 'Get full destination config by slug',
  },
  resolveDestinationRefPath: {
    args: ['slug'],
    returns: 'string | null',
    description: 'Get absolute path to destination reference JSON',
  },
  getDefaultDestination: {
    args: [],
    returns: 'string',
    description: 'Get default destination slug from config',
  },
  getDestinationCurrency: {
    args: ['slug'],
    returns: 'string',
    description: 'Get default currency for a destination (e.g., JPY)',
  },

  // OTA Source Discovery
  getAvailableOtaSources: {
    args: [],
    returns: 'string[]',
    description: 'List all configured OTA source IDs',
  },
  getSupportedOtaSources: {
    args: [],
    returns: 'string[]',
    description: 'List OTA sources with working scrapers (supported=true + scraper_script exists on disk)',
  },
  getOtaSourceConfig: {
    args: ['sourceId'],
    returns: 'OtaSourceConfig | null',
    description: 'Get full OTA source config by ID',
  },
  getOtaSourceCurrency: {
    args: ['sourceId'],
    returns: 'string',
    description: 'Get currency for an OTA source (throws if unknown)',
  },
  getOtaSourcesForMarket: {
    args: ['market'],
    returns: 'OtaSourceConfig[]',
    description: 'Get OTA sources available in a market (e.g., "TW")',
  },
  getOtaSourcesByType: {
    args: ['type'],
    returns: 'OtaSourceConfig[]',
    description: 'Get OTA sources by type (package/flight/hotel)',
  },
} as const;

/**
 * Configuration file paths.
 */
export const CONFIG_FILES = {
  destinations: 'data/destinations.json',
  otaSources: 'data/ota-sources.json',
  constants: 'src/config/constants.ts',
} as const;
