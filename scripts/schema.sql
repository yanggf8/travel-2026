-- =============================================================================
-- travel-2026 Database Schema (Turso / libSQL)
-- Read-only reference — extracted from scripts/turso-migrate.ts
-- Not executed directly; migration script is the source of truth.
-- =============================================================================
--
-- Naming conventions:
--   plan_id      : kebab-case (e.g. 'tokyo-2026', 'kyoto-2026')
--   destination   : snake_case (e.g. 'tokyo_2026', 'kyoto_2026')
--
-- Primary key patterns:
--   Single-entity tables  : (plan_id) — one row per plan
--   Per-destination tables : (plan_id, destination)
--   Per-day tables         : (plan_id, destination, day_number)
--   Per-session tables     : (plan_id, destination, day_number, session_type)
--   Per-offer tables       : (plan_id, destination, id) or (plan_id, destination, offer_id)
--   Ordered child rows     : parent PK + sort_order or candidate_id
--   Autoincrement tables   : id INTEGER PRIMARY KEY AUTOINCREMENT
--
-- JSON suffix convention:
--   Columns ending in _json contain serialised JSON arrays or objects.
--   Prefer normalised child tables where available.
-- =============================================================================


-- =====================
-- Plans (core)
-- =====================

CREATE TABLE IF NOT EXISTS plans (
  plan_id TEXT PRIMARY KEY,
  schema_version TEXT NOT NULL,
  version INTEGER NOT NULL DEFAULT 0,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS plan_metadata (
  plan_id TEXT PRIMARY KEY,
  schema_version TEXT NOT NULL,
  active_destination TEXT,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS plan_destinations (
  plan_id TEXT NOT NULL,
  slug TEXT NOT NULL,
  display_name TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'draft',
  home_address TEXT,
  created_at DATETIME,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, slug)
);

CREATE TABLE IF NOT EXISTS destination_details (
  plan_id TEXT NOT NULL,
  destination TEXT NOT NULL,
  origin_city TEXT,
  region TEXT,
  primary_airport TEXT,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination)
);

CREATE TABLE IF NOT EXISTS destination_cities (
  plan_id TEXT NOT NULL,
  destination TEXT NOT NULL,
  city_slug TEXT NOT NULL,
  display_name TEXT,
  role TEXT NOT NULL,
  nights INTEGER,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination, city_slug)
);


-- =====================
-- Dates & Status
-- =====================

CREATE TABLE IF NOT EXISTS date_anchors (
  plan_id TEXT NOT NULL,
  destination TEXT NOT NULL,
  start_date TEXT NOT NULL,
  end_date TEXT NOT NULL,
  days INTEGER NOT NULL,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination)
);

CREATE TABLE IF NOT EXISTS plan_root_date_anchor (
  plan_id TEXT PRIMARY KEY,
  status TEXT NOT NULL,
  set_out_date TEXT,
  duration_days INTEGER,
  return_date TEXT,
  flexibility_json TEXT,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS process_statuses (
  plan_id TEXT NOT NULL,
  destination TEXT NOT NULL,
  process_id TEXT NOT NULL,
  status TEXT NOT NULL,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination, process_id)
);

CREATE TABLE IF NOT EXISTS cascade_dirty_flags (
  plan_id TEXT NOT NULL,
  destination TEXT NOT NULL,
  process_id TEXT NOT NULL,
  dirty INTEGER NOT NULL DEFAULT 0,
  last_changed DATETIME,
  PRIMARY KEY (plan_id, destination, process_id)
);


-- =====================
-- Offers (per-plan, normalised)
-- =====================

CREATE TABLE IF NOT EXISTS plan_offers (
  plan_id TEXT NOT NULL,
  destination TEXT NOT NULL,
  id TEXT NOT NULL,
  source_id TEXT NOT NULL,
  type TEXT NOT NULL,
  title TEXT,
  price_per_person INTEGER,
  currency TEXT DEFAULT 'TWD',
  availability TEXT,
  url TEXT,
  scraped_at TEXT,
  product_code TEXT,
  duration_days INTEGER,
  price_total INTEGER,
  seats_remaining INTEGER,
  includes_json TEXT,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination, id)
);

CREATE TABLE IF NOT EXISTS plan_offer_flights (
  plan_id TEXT NOT NULL,
  destination TEXT NOT NULL,
  offer_id TEXT NOT NULL,
  direction TEXT NOT NULL CHECK(direction IN ('outbound', 'return')),
  flight_number TEXT,
  airline TEXT,
  airline_code TEXT,
  departure_code TEXT,
  departure_time TEXT,
  arrival_code TEXT,
  arrival_time TEXT,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination, offer_id, direction)
);

CREATE TABLE IF NOT EXISTS plan_offer_hotels (
  plan_id TEXT NOT NULL,
  destination TEXT NOT NULL,
  offer_id TEXT NOT NULL,
  name TEXT,
  slug TEXT,
  area TEXT,
  star_rating INTEGER,
  access_json TEXT,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination, offer_id)
);

CREATE TABLE IF NOT EXISTS plan_offer_date_pricing (
  plan_id TEXT NOT NULL,
  destination TEXT NOT NULL,
  offer_id TEXT NOT NULL,
  date TEXT NOT NULL,
  price INTEGER NOT NULL,
  availability TEXT,
  seats_remaining INTEGER,
  currency TEXT DEFAULT 'TWD',
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination, offer_id, date)
);

CREATE TABLE IF NOT EXISTS plan_offer_best_value (
  plan_id TEXT NOT NULL,
  destination TEXT NOT NULL,
  offer_id TEXT NOT NULL,
  best_date TEXT,
  best_price INTEGER,
  currency TEXT DEFAULT 'TWD',
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination, offer_id)
);

CREATE TABLE IF NOT EXISTS plan_offer_selection (
  plan_id TEXT NOT NULL,
  destination TEXT NOT NULL,
  selected_offer_id TEXT,
  selected_date TEXT,
  selected_at DATETIME,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination)
);

CREATE TABLE IF NOT EXISTS plan_offer_provenance (
  plan_id TEXT NOT NULL,
  destination TEXT NOT NULL,
  source_id TEXT NOT NULL,
  scraped_at TEXT NOT NULL,
  file_path TEXT,
  offer_count INTEGER,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination, source_id, scraped_at)
);

CREATE TABLE IF NOT EXISTS plan_offer_warnings (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  plan_id TEXT NOT NULL,
  destination TEXT NOT NULL,
  offer_id TEXT,
  warning_type TEXT NOT NULL,
  message TEXT NOT NULL,
  created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);


-- =====================
-- Budget
-- =====================

CREATE TABLE IF NOT EXISTS plan_budget (
  plan_id TEXT PRIMARY KEY,
  total_cap INTEGER,
  flight_cap INTEGER,
  accommodation_cap INTEGER,
  daily_cap INTEGER,
  pax INTEGER NOT NULL DEFAULT 1,
  currency TEXT DEFAULT 'TWD',
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
);


-- =====================
-- Flights
-- =====================

-- Legacy blob table (dead — no writes, kept for reference only)
CREATE TABLE IF NOT EXISTS flights (
  plan_id TEXT NOT NULL,
  destination TEXT NOT NULL,
  populated_from TEXT,
  airline TEXT,
  airline_code TEXT,
  outbound_json TEXT,
  return_json TEXT,
  booked_date TEXT,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination)
);

-- Normalised flight data (one row per leg per direction)
CREATE TABLE IF NOT EXISTS flight_legs (
  plan_id TEXT NOT NULL,
  destination TEXT NOT NULL,
  direction TEXT NOT NULL CHECK(direction IN ('outbound', 'return')),
  leg_order INTEGER NOT NULL DEFAULT 0,
  flight_number TEXT,
  airline TEXT,
  airline_code TEXT,
  departure_airport TEXT,
  departure_code TEXT,
  departure_terminal TEXT,
  departure_time TEXT,
  arrival_airport TEXT,
  arrival_code TEXT,
  arrival_terminal TEXT,
  arrival_time TEXT,
  flight_date TEXT,
  populated_from TEXT,
  booked_date TEXT,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination, direction, leg_order)
);


-- =====================
-- Accommodation
-- =====================

CREATE TABLE IF NOT EXISTS hotels (
  plan_id TEXT NOT NULL,
  destination TEXT NOT NULL,
  populated_from TEXT,
  name TEXT,
  name_zh TEXT,
  access_json TEXT,
  check_in TEXT,
  notes TEXT,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination)
);

CREATE TABLE IF NOT EXISTS hotel_access_lines (
  plan_id TEXT NOT NULL,
  destination TEXT NOT NULL,
  sort_order INTEGER NOT NULL,
  line TEXT NOT NULL,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination, sort_order)
);

CREATE TABLE IF NOT EXISTS accommodation_location_zone (
  plan_id TEXT NOT NULL,
  destination TEXT NOT NULL,
  selected_area TEXT,
  source TEXT,
  candidates_json TEXT,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination)
);


-- =====================
-- Airport Transfers
-- =====================

CREATE TABLE IF NOT EXISTS airport_transfers (
  plan_id TEXT NOT NULL,
  destination TEXT NOT NULL,
  direction TEXT NOT NULL CHECK(direction IN ('arrival', 'departure')),
  status TEXT NOT NULL DEFAULT 'planned' CHECK(status IN ('planned', 'booked')),
  selected_json TEXT,
  candidates_json TEXT,
  selected_title TEXT,
  selected_route TEXT,
  selected_duration_min INTEGER,
  selected_price_yen INTEGER,
  selected_schedule TEXT,
  selected_booking_url TEXT,
  selected_notes TEXT,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination, direction)
);

CREATE TABLE IF NOT EXISTS airport_transfer_candidates (
  plan_id TEXT NOT NULL,
  destination TEXT NOT NULL,
  direction TEXT NOT NULL CHECK(direction IN ('arrival', 'departure')),
  candidate_id TEXT NOT NULL,
  title TEXT NOT NULL,
  route TEXT,
  duration_min INTEGER,
  price_yen INTEGER,
  schedule TEXT,
  booking_url TEXT,
  notes TEXT,
  sort_order INTEGER NOT NULL DEFAULT 0,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination, direction, candidate_id)
);

CREATE TABLE IF NOT EXISTS transportation_extras (
  plan_id TEXT NOT NULL,
  destination TEXT NOT NULL,
  source TEXT,
  populated_from TEXT,
  research_notes TEXT,
  home_to_airport_json TEXT,
  airport_to_hotel_json TEXT,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination)
);


-- =====================
-- Itinerary
-- =====================

CREATE TABLE IF NOT EXISTS itinerary_days (
  plan_id TEXT NOT NULL,
  destination TEXT NOT NULL,
  day_number INTEGER NOT NULL,
  date TEXT NOT NULL,
  theme TEXT,
  theme_zh TEXT,
  day_type TEXT NOT NULL CHECK(day_type IN ('arrival', 'full', 'departure')),
  status TEXT NOT NULL DEFAULT 'draft' CHECK(status IN ('draft', 'planned', 'confirmed')),
  weather_label TEXT,
  temp_low_c REAL,
  temp_high_c REAL,
  feels_like_low_c REAL,
  feels_like_high_c REAL,
  precipitation_pct REAL,
  weather_code INTEGER,
  weather_source_id TEXT,
  weather_sourced_at TEXT,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination, day_number)
);

CREATE TABLE IF NOT EXISTS itinerary_sessions (
  plan_id TEXT NOT NULL,
  destination TEXT NOT NULL,
  day_number INTEGER NOT NULL,
  session_type TEXT NOT NULL CHECK(session_type IN ('morning', 'afternoon', 'evening')),
  focus TEXT,
  focus_zh TEXT,
  transit_notes TEXT,
  transit_notes_zh TEXT,
  booking_notes TEXT,
  meals_json TEXT,
  meals_zh_json TEXT,
  activities_zh_json TEXT,
  time_range_start TEXT,
  time_range_end TEXT,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination, day_number, session_type)
);

CREATE TABLE IF NOT EXISTS activities (
  id TEXT PRIMARY KEY,
  plan_id TEXT NOT NULL,
  destination TEXT NOT NULL,
  day_number INTEGER NOT NULL,
  session_type TEXT NOT NULL CHECK(session_type IN ('morning', 'afternoon', 'evening')),
  sort_order INTEGER NOT NULL DEFAULT 0,
  title TEXT NOT NULL,
  area TEXT,
  nearest_station TEXT,
  duration_min INTEGER,
  booking_required INTEGER NOT NULL DEFAULT 0,
  booking_url TEXT,
  booking_status TEXT CHECK(booking_status IN ('not_required', 'pending', 'booked', 'waitlist')),
  booking_ref TEXT,
  book_by TEXT,
  start_time TEXT,
  end_time TEXT,
  is_fixed_time INTEGER NOT NULL DEFAULT 0,
  cost_estimate INTEGER,
  tags_json TEXT,
  notes TEXT,
  priority TEXT NOT NULL DEFAULT 'want' CHECK(priority IN ('must', 'want', 'optional')),
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_activities_session
  ON activities(plan_id, destination, day_number, session_type, sort_order);
CREATE INDEX IF NOT EXISTS idx_activities_booking
  ON activities(plan_id, booking_status);

CREATE TABLE IF NOT EXISTS activity_tags (
  activity_id TEXT NOT NULL,
  tag TEXT NOT NULL,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (activity_id, tag)
);

CREATE TABLE IF NOT EXISTS session_meals (
  plan_id TEXT NOT NULL,
  destination TEXT NOT NULL,
  day_number INTEGER NOT NULL,
  session_type TEXT NOT NULL,
  sort_order INTEGER NOT NULL,
  meal TEXT NOT NULL,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination, day_number, session_type, sort_order)
);

CREATE TABLE IF NOT EXISTS itinerary_metadata (
  plan_id TEXT NOT NULL,
  destination TEXT NOT NULL,
  scaffolded_at DATETIME,
  populated_at DATETIME,
  transit_summary TEXT,
  transit_summary_zh TEXT,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination)
);

CREATE TABLE IF NOT EXISTS day_route_segments (
  plan_id TEXT NOT NULL,
  destination TEXT NOT NULL,
  day_number INTEGER NOT NULL,
  sort_order INTEGER NOT NULL,
  from_place TEXT NOT NULL,
  to_place TEXT NOT NULL,
  mode TEXT NOT NULL,
  PRIMARY KEY (plan_id, destination, day_number, sort_order)
);

CREATE TABLE IF NOT EXISTS day_landmarks (
  plan_id TEXT NOT NULL,
  destination TEXT NOT NULL,
  day_number INTEGER NOT NULL,
  sort_order INTEGER NOT NULL,
  landmark TEXT NOT NULL,
  PRIMARY KEY (plan_id, destination, day_number, sort_order)
);


-- =====================
-- Cascade Engine
-- =====================

CREATE TABLE IF NOT EXISTS cascade_triggers (
  plan_id TEXT NOT NULL,
  trigger_id TEXT NOT NULL,
  event TEXT NOT NULL,
  reset_json TEXT NOT NULL,
  scope TEXT NOT NULL,
  condition_json TEXT,
  action TEXT,
  populate_map_json TEXT,
  set_source TEXT,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, trigger_id)
);

CREATE TABLE IF NOT EXISTS cascade_global_state (
  plan_id TEXT PRIMARY KEY,
  last_cascade_run DATETIME,
  active_dest_last TEXT,
  p1_dirty INTEGER NOT NULL DEFAULT 0,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS plan_schema_contract (
  plan_id TEXT PRIMARY KEY,
  id_convention TEXT NOT NULL,
  currency TEXT NOT NULL DEFAULT 'TWD',
  process_nodes_json TEXT NOT NULL,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS plan_process_precedence (
  plan_id TEXT PRIMARY KEY,
  precedence_json TEXT NOT NULL,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
);


-- =====================
-- Event Log
-- =====================

CREATE TABLE IF NOT EXISTS event_log_state (
  plan_id TEXT PRIMARY KEY,
  session TEXT NOT NULL,
  project TEXT NOT NULL,
  version TEXT NOT NULL,
  current_focus TEXT,
  active_destination TEXT,
  next_actions_json TEXT,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS event_log_global_processes (
  plan_id TEXT NOT NULL,
  process_id TEXT NOT NULL,
  status TEXT NOT NULL,
  events_json TEXT,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, process_id)
);

CREATE TABLE IF NOT EXISTS event_log_destinations (
  plan_id TEXT NOT NULL,
  destination TEXT NOT NULL,
  status TEXT NOT NULL,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination)
);

CREATE TABLE IF NOT EXISTS event_log_dest_processes (
  plan_id TEXT NOT NULL,
  destination TEXT NOT NULL,
  process_id TEXT NOT NULL,
  status TEXT NOT NULL,
  events_json TEXT,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination, process_id)
);

CREATE TABLE IF NOT EXISTS event_log_process_events (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  plan_id TEXT NOT NULL,
  destination TEXT,
  process_id TEXT NOT NULL,
  event_type TEXT NOT NULL,
  event_data TEXT,
  event_at DATETIME DEFAULT CURRENT_TIMESTAMP
);


-- =====================
-- Bookings (normalised)
-- =====================

CREATE TABLE IF NOT EXISTS bookings_current (
  booking_key TEXT PRIMARY KEY,
  trip_id TEXT NOT NULL,
  destination TEXT NOT NULL,
  category TEXT NOT NULL CHECK(category IN ('package','transfer','activity')),
  subtype TEXT,
  title TEXT NOT NULL,
  status TEXT NOT NULL CHECK(status IN ('pending','planned','booked','confirmed','waitlist','skipped','cancelled')),
  reference TEXT,
  book_by TEXT,
  booked_at TEXT,
  source_id TEXT,
  offer_id TEXT,
  selected_date TEXT,
  price_amount INTEGER,
  price_currency TEXT DEFAULT 'TWD',
  origin_path TEXT,
  payload_json TEXT,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_bc_dest ON bookings_current(destination, category);
CREATE INDEX IF NOT EXISTS idx_bc_status ON bookings_current(status);
CREATE INDEX IF NOT EXISTS idx_bc_offer ON bookings_current(offer_id);

CREATE TABLE IF NOT EXISTS bookings_events (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  booking_key TEXT NOT NULL,
  event_type TEXT NOT NULL,
  previous_status TEXT,
  new_status TEXT,
  reference TEXT,
  book_by TEXT,
  amount INTEGER,
  currency TEXT,
  event_data TEXT,
  event_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_be_key ON bookings_events(booking_key, event_at);

-- Legacy bookings table
CREATE TABLE IF NOT EXISTS bookings (
  destination TEXT NOT NULL,
  offer_id TEXT NOT NULL,
  selected_date TEXT NOT NULL,
  price_per_person INTEGER,
  price_total INTEGER,
  currency TEXT DEFAULT 'TWD',
  status TEXT CHECK(status IN ('selected', 'booked', 'confirmed')),
  source_id TEXT,
  hotel_name TEXT,
  airline TEXT,
  flight_out TEXT,
  flight_return TEXT,
  selected_at DATETIME,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (destination, offer_id)
);


-- =====================
-- Global Destination Config (not plan-scoped)
-- =====================

CREATE TABLE IF NOT EXISTS destination_config (
  slug TEXT PRIMARY KEY,
  display_name TEXT NOT NULL,
  ref_id TEXT,
  ref_path TEXT,
  timezone TEXT NOT NULL DEFAULT 'Asia/Tokyo',
  currency TEXT NOT NULL DEFAULT 'JPY',
  markets_json TEXT,
  primary_airports_json TEXT,
  language TEXT DEFAULT 'ja',
  origin TEXT DEFAULT 'taiwan',
  lat REAL,
  lon REAL,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS origin_config (
  slug TEXT PRIMARY KEY,
  country_code TEXT,
  currency TEXT,
  timezone TEXT,
  holiday_calendar TEXT,
  primary_airports_json TEXT,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS global_config (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
);


-- =====================
-- Operation Tracking
-- =====================

CREATE TABLE IF NOT EXISTS operation_runs (
  run_id TEXT PRIMARY KEY,
  plan_id TEXT NOT NULL,
  command_type TEXT NOT NULL,
  command_summary TEXT,
  status TEXT NOT NULL DEFAULT 'started'
    CHECK(status IN ('started', 'completed', 'failed')),
  version_before INTEGER NOT NULL,
  version_after INTEGER,
  error_message TEXT,
  started_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
  completed_at DATETIME,
  idempotency_key TEXT
);

CREATE INDEX IF NOT EXISTS idx_operation_runs_plan
  ON operation_runs(plan_id, started_at DESC);
CREATE UNIQUE INDEX IF NOT EXISTS idx_operation_runs_idempotency
  ON operation_runs(plan_id, idempotency_key);


-- =====================
-- OTA Sources Config
-- =====================

CREATE TABLE IF NOT EXISTS ota_sources (
  source_id    TEXT PRIMARY KEY,
  name         TEXT NOT NULL,
  type_json    TEXT,              -- JSON array: ["package","flight","hotel"]
  status       TEXT DEFAULT 'active',
  scraper_script TEXT,
  regions_json TEXT,              -- JSON array of supported regions
  url_template TEXT,
  notes        TEXT,
  updated_at   DATETIME DEFAULT CURRENT_TIMESTAMP
);
