-- =============================================================================
-- travel-2026 Database Schema (Turso / libSQL)
-- AUTO-GENERATED from the live DB via scripts/turso-migrate.ts's output.
-- Regenerate: npx ts-node scripts/gen-schema-sql.ts  (do NOT hand-edit)
-- Source of truth = scripts/turso-migrate.ts; this mirrors the applied schema.
-- Generated: 2026-06-29
-- Tables: 115 | Indexes: 21
-- =============================================================================
--
-- Naming conventions:
--   plan_id      : kebab-case (e.g. 'tokyo-2026')
--   destination  : snake_case (e.g. 'tokyo_2026')
--
-- NOTE (de-JSON program, 2026-06): no JSON-encoded values are stored in any
-- column. Former *_json columns were re-normalized into child tables / typed
-- columns / *_text fields. See memory no-json-in-rdb.
-- =============================================================================

CREATE TABLE accommodation_location_zone (
  plan_id TEXT NOT NULL, destination TEXT NOT NULL,
  selected_area TEXT, source TEXT, updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination)
);

CREATE TABLE activities (
    id TEXT PRIMARY KEY,
    plan_id TEXT NOT NULL,
    destination TEXT NOT NULL,
    poi_id TEXT,
    day_number INTEGER NOT NULL,
    session_type TEXT NOT NULL CHECK(session_type IN ('morning','noon','afternoon','evening')),
    sort_order INTEGER NOT NULL DEFAULT 0,
    title TEXT NOT NULL,
    area TEXT,
    nearest_station TEXT,
    duration_min INTEGER,
    booking_required INTEGER NOT NULL DEFAULT 0,
    booking_url TEXT,
    booking_status TEXT CHECK(booking_status IN ('not_required','pending','booked','waitlist')),
    booking_ref TEXT,
    book_by TEXT,
    start_time TEXT,
    end_time TEXT,
    is_fixed_time INTEGER NOT NULL DEFAULT 0,
    cost_estimate INTEGER,
    notes TEXT,
    priority TEXT NOT NULL DEFAULT 'want' CHECK(priority IN ('must','want','optional')),
    source TEXT NOT NULL DEFAULT 'confirmed' CHECK(source IN ('confirmed','ai_recommended')),
    updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
  );

CREATE TABLE activity_tags (
  activity_id TEXT NOT NULL, tag TEXT NOT NULL,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (activity_id, tag)
);

CREATE TABLE airlines (
    code TEXT PRIMARY KEY,
    name TEXT,
    type TEXT,
    hand_baggage_kg INTEGER,
    checked_bag_included INTEGER,
    checked_bag_kg INTEGER,
    checked_bag_cost_twd INTEGER,
    checked_bag_cost_jpy INTEGER,
    classification_baggage_note TEXT,
    classification_meal TEXT,
    source_url TEXT,
    fetched_at TEXT,
    confidence TEXT
  );

CREATE TABLE airport_transfer_candidates (
  plan_id TEXT NOT NULL, destination TEXT NOT NULL,
  direction TEXT NOT NULL CHECK(direction IN ('arrival', 'departure')),
  candidate_id TEXT NOT NULL,
  title TEXT NOT NULL, route TEXT, duration_min INTEGER, price_yen INTEGER,
  schedule TEXT, booking_url TEXT, notes TEXT, sort_order INTEGER NOT NULL DEFAULT 0,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination, direction, candidate_id)
);

CREATE TABLE airport_transfers (
  plan_id TEXT NOT NULL,
  destination TEXT NOT NULL,
  direction TEXT NOT NULL CHECK(direction IN ('arrival', 'departure')),
  status TEXT NOT NULL DEFAULT 'planned' CHECK(status IN ('planned', 'booked')),
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP, selected_title TEXT, selected_route TEXT, selected_duration_min INTEGER, selected_price_yen INTEGER, selected_schedule TEXT, selected_booking_url TEXT, selected_notes TEXT, selected_id TEXT,
  PRIMARY KEY (plan_id, destination, direction)
);

CREATE TABLE booking_type_rules (
    booking_type TEXT NOT NULL, sort_order INTEGER NOT NULL, rule TEXT NOT NULL,
    PRIMARY KEY (booking_type, sort_order)
  );

CREATE TABLE booking_types (
    slug TEXT PRIMARY KEY,
    name_zh TEXT,
    description TEXT,
    source_url TEXT,
    fetched_at TEXT,
    confidence TEXT
  );

CREATE TABLE bookings (
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

CREATE TABLE bookings_current (
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
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE bookings_current_payload (
    booking_key TEXT NOT NULL, sort_order INTEGER NOT NULL,
    key TEXT NOT NULL, value TEXT NOT NULL,
    PRIMARY KEY (booking_key, sort_order)
  );

CREATE TABLE bookings_event_data (
    booking_key TEXT NOT NULL, event_at TEXT NOT NULL, sort_order INTEGER NOT NULL,
    key TEXT NOT NULL, value TEXT NOT NULL,
    PRIMARY KEY (booking_key, event_at, sort_order)
  );

CREATE TABLE bookings_events (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  booking_key TEXT NOT NULL,
  event_type TEXT NOT NULL,
  previous_status TEXT,
  new_status TEXT,
  reference TEXT,
  book_by TEXT,
  amount INTEGER,
  currency TEXT,
  event_at DATETIME DEFAULT CURRENT_TIMESTAMP
, event_data_text TEXT);

CREATE TABLE captures (capture_id TEXT PRIMARY KEY, source_id TEXT NOT NULL, url TEXT, title TEXT, captured_at TEXT NOT NULL, raw_text TEXT NOT NULL);

CREATE TABLE cascade_dirty_flags (
  plan_id TEXT NOT NULL,
  destination TEXT NOT NULL,
  process_id TEXT NOT NULL,
  dirty INTEGER NOT NULL DEFAULT 0,
  last_changed DATETIME,
  PRIMARY KEY (plan_id, destination, process_id)
);

CREATE TABLE cascade_global_state (
  plan_id TEXT PRIMARY KEY,
  last_cascade_run DATETIME, active_dest_last TEXT, p1_dirty INTEGER NOT NULL DEFAULT 0,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE cascade_trigger_populate_map (
    trigger_id TEXT NOT NULL, source_path TEXT NOT NULL, target_path TEXT NOT NULL,
    PRIMARY KEY (trigger_id, source_path)
  );

CREATE TABLE cascade_trigger_resets (
    trigger_id TEXT NOT NULL, sort_order INTEGER NOT NULL, target TEXT NOT NULL,
    PRIMARY KEY (trigger_id, sort_order)
  );

CREATE TABLE cascade_triggers (
  plan_id TEXT NOT NULL, trigger_id TEXT NOT NULL,
  event TEXT NOT NULL, scope TEXT NOT NULL,
  action TEXT, set_source TEXT,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP, condition_field TEXT, condition_changed INTEGER,
  PRIMARY KEY (plan_id, trigger_id)
);

CREATE TABLE comparison_rules (
    id INTEGER PRIMARY KEY,
    rule TEXT,
    sort_order INTEGER,
    source_url TEXT,
    fetched_at TEXT,
    confidence TEXT
  );

CREATE TABLE date_anchors (
  plan_id TEXT NOT NULL,
  destination TEXT NOT NULL,
  start_date TEXT NOT NULL,
  end_date TEXT NOT NULL,
  days INTEGER NOT NULL,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination)
);

CREATE TABLE day_landmarks (
  plan_id TEXT NOT NULL, destination TEXT NOT NULL,
  day_number INTEGER NOT NULL, sort_order INTEGER NOT NULL,
  landmark TEXT NOT NULL,
  PRIMARY KEY (plan_id, destination, day_number, sort_order)
);

CREATE TABLE day_route_segments (
  plan_id TEXT NOT NULL, destination TEXT NOT NULL,
  day_number INTEGER NOT NULL, sort_order INTEGER NOT NULL,
  from_place TEXT NOT NULL, to_place TEXT NOT NULL,
  mode TEXT NOT NULL, duration_min INTEGER, notes TEXT, start_time TEXT,
  source TEXT NOT NULL DEFAULT 'confirmed' CHECK(source IN ('confirmed','ai_recommended')),
  PRIMARY KEY (plan_id, destination, day_number, sort_order)
);

CREATE TABLE "days" (
  plan_id TEXT NOT NULL,
  destination TEXT NOT NULL,
  day_number INTEGER NOT NULL,
  date TEXT NOT NULL,
  theme TEXT,
  day_type TEXT NOT NULL CHECK(day_type IN ('arrival', 'full', 'departure')),
  status TEXT NOT NULL DEFAULT 'draft' CHECK(status IN ('draft', 'planned', 'confirmed')),
  weather_label TEXT,
  temp_low_c REAL,
  temp_high_c REAL,
  precipitation_pct REAL,
  weather_code INTEGER,
  weather_source_id TEXT,
  weather_sourced_at TEXT,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP, feels_like_low_c REAL, feels_like_high_c REAL, theme_zh TEXT,
  PRIMARY KEY (plan_id, destination, day_number)
);

CREATE TABLE destination_airports (
    slug TEXT, airport TEXT, sort_order INTEGER,
    PRIMARY KEY (slug, sort_order)
  );

CREATE TABLE destination_area_best_for (
    slug TEXT, area_id TEXT, tag TEXT, sort_order INTEGER,
    PRIMARY KEY (slug, area_id, sort_order)
  );

CREATE TABLE destination_area_stations (
    slug TEXT, area_id TEXT, station TEXT, sort_order INTEGER,
    PRIMARY KEY (slug, area_id, sort_order)
  );

CREATE TABLE destination_areas (
    slug TEXT,
    area_id TEXT,
    name TEXT,
    type TEXT,
    vibe TEXT,
    source_url TEXT,
    fetched_at TEXT,
    confidence TEXT,
    PRIMARY KEY (slug, area_id)
  );

CREATE TABLE destination_cities (
  plan_id TEXT NOT NULL, destination TEXT NOT NULL, city_slug TEXT NOT NULL,
  role TEXT NOT NULL, nights INTEGER,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP, display_name TEXT,
  PRIMARY KEY (plan_id, destination, city_slug)
);

CREATE TABLE destination_cluster_pois (
    slug TEXT, cluster_id TEXT, poi_id TEXT, sort_order INTEGER,
    PRIMARY KEY (slug, cluster_id, sort_order)
  );

CREATE TABLE destination_clusters (
    slug TEXT,
    cluster_id TEXT,
    name TEXT,
    description TEXT,
    duration_min INTEGER,
    best_area TEXT,
    source_url TEXT,
    fetched_at TEXT,
    confidence TEXT,
    PRIMARY KEY (slug, cluster_id)
  );

CREATE TABLE destination_config (
  slug TEXT PRIMARY KEY,
  display_name TEXT NOT NULL,
  ref_id TEXT,
  ref_path TEXT,
  timezone TEXT NOT NULL DEFAULT 'Asia/Tokyo',
  currency TEXT NOT NULL DEFAULT 'JPY',
  language TEXT DEFAULT 'ja',
  origin TEXT DEFAULT 'taiwan',
  lat REAL,
  lon REAL,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE destination_details (
  plan_id TEXT NOT NULL, destination TEXT NOT NULL,
  origin_city TEXT, region TEXT, primary_airport TEXT,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination)
);

CREATE TABLE destination_markets (
    slug TEXT, market TEXT, sort_order INTEGER,
    PRIMARY KEY (slug, sort_order)
  );

CREATE TABLE destination_poi_tags (
    slug TEXT, poi_id TEXT, tag TEXT, sort_order INTEGER,
    PRIMARY KEY (slug, poi_id, sort_order)
  );

CREATE TABLE destination_pois (
    slug TEXT,
    poi_id TEXT,
    title TEXT,
    area TEXT,
    nearest_station TEXT,
    duration_min INTEGER,
    booking_required INTEGER,
    booking_url TEXT,
    cost_estimate INTEGER,
    notes TEXT,
    hours TEXT,
    address TEXT,
    source_url TEXT,
    fetched_at TEXT,
    confidence TEXT,
    lat REAL,
    lon REAL,
    PRIMARY KEY (slug, poi_id)
  );

CREATE TABLE destination_tips (
    slug TEXT, tip TEXT, sort_order INTEGER,
    PRIMARY KEY (slug, sort_order)
  );

CREATE TABLE destination_transit (
    slug TEXT,
    pair_key TEXT,
    kind TEXT,
    minutes INTEGER,
    line TEXT,
    station_from TEXT,
    station_to TEXT,
    source_url TEXT,
    fetched_at TEXT,
    confidence TEXT,
    PRIMARY KEY (slug, pair_key)
  );

CREATE TABLE destinations (
    slug TEXT PRIMARY KEY,
    display_name TEXT NOT NULL,
    currency TEXT DEFAULT 'JPY',
    timezone TEXT,
    primary_airports TEXT,  -- JSON array
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE event_log_dest_processes (
  plan_id TEXT NOT NULL, destination TEXT NOT NULL, process_id TEXT NOT NULL,
  status TEXT NOT NULL, updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination, process_id)
);

CREATE TABLE event_log_destinations (
  plan_id TEXT NOT NULL, destination TEXT NOT NULL,
  status TEXT NOT NULL,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination)
);

CREATE TABLE event_log_global_processes (
  plan_id TEXT NOT NULL, process_id TEXT NOT NULL,
  status TEXT NOT NULL, updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, process_id)
);

CREATE TABLE event_log_next_actions (
    plan_id TEXT NOT NULL, sort_order INTEGER NOT NULL, action TEXT NOT NULL,
    PRIMARY KEY (plan_id, sort_order)
  );

CREATE TABLE event_log_state (
  plan_id TEXT PRIMARY KEY,
  session TEXT NOT NULL, project TEXT NOT NULL, version TEXT NOT NULL,
  current_focus TEXT, active_destination TEXT, updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    event_type TEXT NOT NULL,
    destination TEXT,
    process TEXT,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
, external_id TEXT, data_text TEXT);

CREATE TABLE flight_legs (
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

CREATE TABLE global_config (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE holidays (
      country TEXT NOT NULL,           -- 'taiwan', 'japan', etc. (lowercase canonical name)
      date TEXT NOT NULL,              -- ISO YYYY-MM-DD
      day_of_week TEXT,                -- 一/二/三/四/五/六/日 for taiwan, or English
      is_holiday INTEGER NOT NULL,     -- 0 = workday, 1 = makeup workday, 2 = holiday/weekend
      name TEXT,                       -- holiday name in source language; NULL on plain weekends
      source_url TEXT NOT NULL,        -- where this row was fetched from
      source_label TEXT NOT NULL,      -- e.g. 'DGPA 行政院人事行政總處 115年辦公日曆表'
      fetched_at TEXT NOT NULL, year INTEGER, confidence TEXT,        -- ISO timestamp of fetch
      PRIMARY KEY (country, date)
    );

CREATE TABLE hotel_access_lines (
  plan_id TEXT NOT NULL, destination TEXT NOT NULL,
  sort_order INTEGER NOT NULL, line TEXT NOT NULL,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination, sort_order)
);

CREATE TABLE hotel_area_keywords (
    region TEXT NOT NULL, area_type TEXT NOT NULL, sort_order INTEGER NOT NULL, keyword TEXT NOT NULL,
    PRIMARY KEY (region, area_type, sort_order)
  );

CREATE TABLE hotel_areas (
    region TEXT,
    area_type TEXT,
    source_url TEXT,
    fetched_at TEXT,
    confidence TEXT,
    PRIMARY KEY (region, area_type)
  );

CREATE TABLE hotels (
  plan_id TEXT NOT NULL,
  destination TEXT NOT NULL,
  populated_from TEXT,
  name TEXT,
  check_in TEXT,
  notes TEXT,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP, name_zh TEXT,
  PRIMARY KEY (plan_id, destination)
);

CREATE TABLE itinerary_metadata (
  plan_id TEXT NOT NULL, destination TEXT NOT NULL,
  scaffolded_at DATETIME, populated_at DATETIME, updated_at DATETIME DEFAULT CURRENT_TIMESTAMP, transit_hotel_station TEXT, transit_hotel_station_zh TEXT,
  PRIMARY KEY (plan_id, destination)
);

CREATE TABLE itinerary_transit_key_lines (
    plan_id TEXT NOT NULL, destination TEXT NOT NULL, lang TEXT NOT NULL,
    sort_order INTEGER NOT NULL, line TEXT NOT NULL,
    PRIMARY KEY (plan_id, destination, lang, sort_order)
  );

CREATE TABLE location_zone_candidates (
    plan_id TEXT NOT NULL, destination TEXT NOT NULL,
    sort_order INTEGER NOT NULL, slug TEXT NOT NULL, display_name TEXT,
    pros_text TEXT, cons_text TEXT,
    PRIMARY KEY (plan_id, destination, sort_order)
  );

CREATE TABLE "offers" (id TEXT NOT NULL, source_id TEXT NOT NULL, type TEXT CHECK(type IN ('package', 'flight', 'hotel')), name TEXT, price_per_person INTEGER, currency TEXT DEFAULT 'TWD', region TEXT, destination TEXT, departure_date TEXT, return_date TEXT, nights INTEGER, availability TEXT CHECK(availability IN ('available', 'sold_out', 'limited')), hotel_name TEXT, hotel_area TEXT, airline TEXT, flight_outbound TEXT, flight_return TEXT, includes TEXT, scraped_at DATETIME NOT NULL, created_at DATETIME DEFAULT CURRENT_TIMESTAMP, source_file TEXT, capture_id TEXT, produced_by_job_id TEXT, produced_by_attempt_id TEXT, parser_method TEXT CHECK(parser_method IS NULL OR parser_method IN ('agent_parse','regex')), capture_checksum TEXT, parser_rule_checksum TEXT, normalizer_version TEXT, PRIMARY KEY (id, scraped_at));

CREATE TABLE operation_runs (
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

CREATE TABLE origin_airports (
    slug TEXT, airport TEXT, sort_order INTEGER,
    PRIMARY KEY (slug, sort_order)
  );

CREATE TABLE origin_config (
  slug TEXT PRIMARY KEY,
  country_code TEXT,
  currency TEXT,
  timezone TEXT,
  holiday_calendar TEXT,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE ota_attempts (
  attempt_id TEXT PRIMARY KEY,
  job_id TEXT NOT NULL,
  attempt_no INTEGER NOT NULL,
  claim_token TEXT NOT NULL,
  outcome TEXT NOT NULL CHECK(outcome IN ('succeeded','failed','blocked')),
  capture_id TEXT,
  candidate_count INTEGER NOT NULL DEFAULT 0,
  inserted_count INTEGER NOT NULL DEFAULT 0,
  deduped_count INTEGER NOT NULL DEFAULT 0,
  error_detail TEXT,
  started_at TEXT NOT NULL,
  finished_at TEXT,
  UNIQUE(job_id, attempt_no)
);

CREATE TABLE ota_job_params (
  job_id TEXT NOT NULL,
  param_key TEXT NOT NULL CHECK(param_key IN ('depart_date','return_date','nights','pax','region_code','region_label')),
  param_value TEXT NOT NULL,
  PRIMARY KEY (job_id, param_key)
);

CREATE TABLE ota_jobs (
  job_id TEXT PRIMARY KEY,
  source_id TEXT NOT NULL,
  product_type TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'queued' CHECK(status IN ('queued','claimed','running','succeeded','failed','blocked')),
  claimed_by TEXT,
  claimed_at TEXT,
  claim_token TEXT,
  lease_expires_at TEXT,
  heartbeat_at TEXT,
  attempts INTEGER NOT NULL DEFAULT 0,
  max_attempts INTEGER NOT NULL DEFAULT 3,
  next_retry_at TEXT,
  blocked_reason_code TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  CHECK(status!='blocked' OR blocked_reason_code IS NOT NULL)
);

CREATE TABLE ota_observations (
  observation_id TEXT PRIMARY KEY,
  source_id TEXT NOT NULL,
  product_type TEXT,
  job_id TEXT,
  attempt_id TEXT,
  observation_type TEXT NOT NULL CHECK(observation_type IN ('block','captcha','login_wall','render_error','rate_limit','parse_warning','freshness','empty_result')),
  block_reason_code TEXT,
  severity TEXT NOT NULL DEFAULT 'warn' CHECK(severity IN ('info','warn','error')),
  http_status INTEGER,
  field_name TEXT,
  selector TEXT,
  expected_value TEXT,
  observed_value TEXT,
  duration_ms INTEGER,
  freshness_reference_at TEXT,
  detail TEXT,
  observed_at TEXT NOT NULL
);

CREATE TABLE ota_source_regions (
      source_id TEXT, region TEXT, PRIMARY KEY (source_id, region)
    );

CREATE TABLE ota_source_types (
      source_id TEXT, type TEXT, PRIMARY KEY (source_id, type)
    );

CREATE TABLE ota_sources (
  source_id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  status TEXT DEFAULT 'active',
  scraper_script TEXT,
  url_template TEXT,
  notes TEXT,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE parser_rules (
                    source_id TEXT NOT NULL,
                    product_type TEXT NOT NULL DEFAULT 'fit',
                    date_range_rx TEXT NOT NULL,
                    nights_rx TEXT NOT NULL,
                    nights_is_days INTEGER DEFAULT 0,
                    price_marker TEXT NOT NULL,
                    price_amount_rx TEXT NOT NULL,
                    price_basis TEXT DEFAULT 'total',
                    pax_divisor INTEGER DEFAULT 2,
                    flight_rx TEXT NOT NULL,
                    hotel_anchor_rx TEXT NOT NULL,
                    currency TEXT DEFAULT 'TWD',
                    has_custom_parser INTEGER DEFAULT 0,
                    source_url TEXT,
                    fetched_at TEXT,
                    airline_rx TEXT DEFAULT '',
                    hotel_name_rx TEXT DEFAULT '',
                    PRIMARY KEY (source_id, product_type)
                 );

CREATE TABLE plan_budget (
  plan_id TEXT PRIMARY KEY,
  total_cap INTEGER, flight_cap INTEGER, accommodation_cap INTEGER, daily_cap INTEGER,
  pax INTEGER NOT NULL DEFAULT 1, currency TEXT DEFAULT 'TWD',
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE plan_date_anchor_flex_dates (
    plan_id TEXT NOT NULL, kind TEXT NOT NULL, sort_order INTEGER NOT NULL, date TEXT NOT NULL,
    PRIMARY KEY (plan_id, kind, sort_order)
  );

CREATE TABLE plan_destinations (
  plan_id TEXT NOT NULL, slug TEXT NOT NULL, display_name TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'draft',
  created_at DATETIME, updated_at DATETIME DEFAULT CURRENT_TIMESTAMP, home_address TEXT,
  PRIMARY KEY (plan_id, slug)
);

CREATE TABLE plan_event_data (
    plan_id TEXT NOT NULL, scope TEXT NOT NULL, destination TEXT NOT NULL DEFAULT '',
    process_id TEXT NOT NULL DEFAULT '', sort_order INTEGER NOT NULL,
    key TEXT NOT NULL, value TEXT,
    PRIMARY KEY (plan_id, scope, destination, process_id, sort_order, key)
  );

CREATE TABLE plan_events (
    plan_id TEXT NOT NULL,
    scope TEXT NOT NULL,           -- 'timeline' | 'global_process' | 'dest_process'
    destination TEXT NOT NULL DEFAULT '',  -- '' for timeline + global_process
    process_id TEXT NOT NULL DEFAULT '',   -- '' for timeline-without-process
    sort_order INTEGER NOT NULL,
    event TEXT, event_at TEXT, from_state TEXT, to_state TEXT,
    PRIMARY KEY (plan_id, scope, destination, process_id, sort_order)
  );

CREATE TABLE plan_metadata (
  plan_id TEXT PRIMARY KEY,
  schema_version TEXT NOT NULL,
  active_destination TEXT,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE plan_offer_best_value (
  plan_id TEXT NOT NULL, destination TEXT NOT NULL, offer_id TEXT NOT NULL,
  best_date TEXT, best_price INTEGER, currency TEXT DEFAULT 'TWD',
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination, offer_id)
);

CREATE TABLE plan_offer_date_pricing (
  plan_id TEXT NOT NULL, destination TEXT NOT NULL, offer_id TEXT NOT NULL,
  date TEXT NOT NULL, price INTEGER NOT NULL, availability TEXT, seats_remaining INTEGER,
  currency TEXT DEFAULT 'TWD',
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination, offer_id, date)
);

CREATE TABLE plan_offer_flights (
  plan_id TEXT NOT NULL, destination TEXT NOT NULL, offer_id TEXT NOT NULL,
  direction TEXT NOT NULL CHECK(direction IN ('outbound', 'return')),
  flight_number TEXT, airline TEXT, airline_code TEXT,
  departure_code TEXT, departure_time TEXT, arrival_code TEXT, arrival_time TEXT,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination, offer_id, direction)
);

CREATE TABLE plan_offer_group_meta (
      plan_id TEXT NOT NULL,
      destination TEXT NOT NULL,
      offer_id TEXT NOT NULL,
      meals_included_count INTEGER,
      departure_status TEXT,
      seats_available INTEGER,
      min_group_size INTEGER,
      group_size_cap INTEGER,
      source_offer_run_id TEXT,
      source_offer_id TEXT,
      PRIMARY KEY (plan_id, destination, offer_id),
      FOREIGN KEY (plan_id, destination, offer_id) REFERENCES plan_offers(plan_id, destination, id)
    );

CREATE TABLE plan_offer_hotel_access (
    plan_id TEXT NOT NULL, destination TEXT NOT NULL, offer_id TEXT NOT NULL,
    sort_order INTEGER NOT NULL, line TEXT NOT NULL,
    PRIMARY KEY (plan_id, destination, offer_id, sort_order)
  );

CREATE TABLE plan_offer_hotels (
  plan_id TEXT NOT NULL, destination TEXT NOT NULL, offer_id TEXT NOT NULL,
  name TEXT, slug TEXT, area TEXT, star_rating INTEGER, updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination, offer_id)
);

CREATE TABLE plan_offer_includes (
    plan_id TEXT NOT NULL, destination TEXT NOT NULL, offer_id TEXT NOT NULL,
    sort_order INTEGER NOT NULL, item TEXT NOT NULL,
    PRIMARY KEY (plan_id, destination, offer_id, sort_order)
  );

CREATE TABLE plan_offer_provenance (
  plan_id TEXT NOT NULL, destination TEXT NOT NULL,
  source_id TEXT NOT NULL, scraped_at TEXT NOT NULL,
  file_path TEXT,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP, offer_count INTEGER,
  PRIMARY KEY (plan_id, destination, source_id, scraped_at)
);

CREATE TABLE plan_offer_selection (
  plan_id TEXT NOT NULL, destination TEXT NOT NULL,
  selected_offer_id TEXT, selected_date TEXT, selected_at DATETIME,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination)
);

CREATE TABLE plan_offer_warnings (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  plan_id TEXT NOT NULL, destination TEXT NOT NULL, offer_id TEXT,
  warning_type TEXT NOT NULL, message TEXT NOT NULL,
  created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE plan_offers (
  plan_id TEXT NOT NULL, destination TEXT NOT NULL, id TEXT NOT NULL,
  source_id TEXT NOT NULL, type TEXT NOT NULL, title TEXT,
  price_per_person INTEGER, currency TEXT DEFAULT 'TWD', availability TEXT,
  url TEXT, scraped_at TEXT, product_code TEXT, duration_days INTEGER,
  price_total INTEGER, seats_remaining INTEGER, updated_at DATETIME DEFAULT CURRENT_TIMESTAMP, package_subtype TEXT,
  PRIMARY KEY (plan_id, destination, id)
);

CREATE TABLE plan_process_precedence (
  plan_id TEXT PRIMARY KEY,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE plan_process_precedence_entries (
    plan_id TEXT NOT NULL, name TEXT NOT NULL,
    primary_process TEXT, mode TEXT, fallback_text TEXT, rules_text TEXT,
    PRIMARY KEY (plan_id, name)
  );

CREATE TABLE plan_root_date_anchor (
  plan_id TEXT PRIMARY KEY,
  status TEXT NOT NULL, set_out_date TEXT, duration_days INTEGER, return_date TEXT,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
, flex_date_flexible INTEGER, flex_reason TEXT);

CREATE TABLE plan_schema_contract (
  plan_id TEXT PRIMARY KEY,
  id_convention TEXT NOT NULL, currency TEXT NOT NULL DEFAULT 'TWD',
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE plan_schema_contract_nodes (
    plan_id TEXT NOT NULL, sort_order INTEGER NOT NULL, node TEXT NOT NULL,
    PRIMARY KEY (plan_id, sort_order)
  );

CREATE TABLE "plans" (
  plan_id TEXT PRIMARY KEY,
  schema_version TEXT NOT NULL,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
, version INTEGER NOT NULL DEFAULT 0);

CREATE TABLE platform_behavior_baggage_labels (
    platform TEXT NOT NULL, label TEXT NOT NULL, description TEXT,
    PRIMARY KEY (platform, label)
  );

CREATE TABLE platform_behavior_quirks (
    platform TEXT NOT NULL, sort_order INTEGER NOT NULL, quirk TEXT NOT NULL,
    PRIMARY KEY (platform, sort_order)
  );

CREATE TABLE platform_behaviors (
    platform TEXT PRIMARY KEY,
    currency TEXT,
    price_display TEXT,
    source_url TEXT,
    fetched_at TEXT,
    confidence TEXT
  );

CREATE TABLE process_statuses (
  plan_id TEXT NOT NULL,
  destination TEXT NOT NULL,
  process_id TEXT NOT NULL,
  status TEXT NOT NULL,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination, process_id)
);

CREATE TABLE session_activities_zh (
    plan_id TEXT NOT NULL, destination TEXT NOT NULL,
    day_number INTEGER NOT NULL, session_type TEXT NOT NULL,
    sort_order INTEGER NOT NULL, activity TEXT NOT NULL,
    PRIMARY KEY (plan_id, destination, day_number, session_type, sort_order)
  );

CREATE TABLE session_meals (
  plan_id TEXT NOT NULL, destination TEXT NOT NULL,
  day_number INTEGER NOT NULL, session_type TEXT NOT NULL,
  sort_order INTEGER NOT NULL, meal TEXT NOT NULL,
  source TEXT NOT NULL DEFAULT 'confirmed' CHECK(source IN ('confirmed','ai_recommended')),
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination, day_number, session_type, sort_order)
);

CREATE TABLE shaping_candidate_flights (
      candidate_id TEXT NOT NULL,
      direction TEXT NOT NULL,
      airline TEXT,
      depart_time TEXT,
      arrive_time TEXT,
      duration TEXT,
      nonstop INTEGER,
      price_total_twd INTEGER,
      PRIMARY KEY (candidate_id, direction)
    );

CREATE TABLE shaping_candidates (
      candidate_id TEXT PRIMARY KEY,
      run_id TEXT NOT NULL,
      dest_code TEXT NOT NULL,
      depart_date TEXT NOT NULL,
      return_date TEXT NOT NULL,
      nights INTEGER NOT NULL,
      flight_total_twd INTEGER,
      leave_days INTEGER,
      rank INTEGER,
      verdict TEXT,
      adopted_plan_id TEXT
    );

CREATE TABLE shaping_research_artifact_notes (
    artifact_id TEXT NOT NULL, sort_order INTEGER NOT NULL,
    key TEXT NOT NULL, value TEXT NOT NULL,
    PRIMARY KEY (artifact_id, sort_order)
  );

CREATE TABLE shaping_research_artifacts (
  artifact_id TEXT PRIMARY KEY, run_id TEXT, destination_slug TEXT, artifact_kind TEXT,
  original_filename TEXT, raw_text TEXT, observed_by TEXT, observed_at TEXT, imported_at TEXT
);

CREATE TABLE shaping_research_destinations (
      run_id TEXT NOT NULL,
      dest_code TEXT NOT NULL,
      dest_label TEXT NOT NULL,
      sort_order INTEGER NOT NULL,
      PRIMARY KEY (run_id, dest_code)
    );

CREATE TABLE shaping_research_durations (
      run_id TEXT NOT NULL,
      nights INTEGER NOT NULL,
      duration_days INTEGER NOT NULL,
      PRIMARY KEY (run_id, nights)
    );

CREATE TABLE shaping_research_runs (
      run_id TEXT PRIMARY KEY,
      origin_code TEXT NOT NULL,
      pax INTEGER NOT NULL,
      window_start TEXT NOT NULL,
      window_end TEXT NOT NULL,
      currency TEXT NOT NULL,
      exchange_rate_usd_twd REAL NOT NULL,
      status TEXT NOT NULL,
      created_at TEXT NOT NULL,
      updated_at TEXT NOT NULL
    );

CREATE TABLE shaping_rules (
    shaping_id INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id TEXT NOT NULL,
    aspect TEXT NOT NULL,             -- 'date' | 'channel' | 'mobility' | 'lodging' | 'budget' | 'activity' | 'general'
    role TEXT NOT NULL,               -- 'hard_constraint' | 'soft_preference' | 'search_directive' | 'observed_signal' | 'hypothesis'
    kind TEXT NOT NULL,               -- e.g. 'return_no_later_than', 'exclude_depart', 'preferred_depart', 'exclude_source', 'no_car', 'location_requirement'
    value_text TEXT,
    value_date TEXT,
    value_integer INTEGER,
    notes TEXT,
    created_at TEXT NOT NULL
  );

CREATE TABLE shaping_scrape_attempts (
      run_id TEXT NOT NULL,
      dest_code TEXT NOT NULL,
      nights INTEGER NOT NULL,
      status TEXT NOT NULL,
      candidate_count INTEGER,
      error TEXT,
      attempted_at TEXT,
      PRIMARY KEY (run_id, dest_code, nights)
    );

CREATE TABLE shaping_selected_offer_notes (
    selection_id TEXT NOT NULL, source TEXT NOT NULL, sort_order INTEGER NOT NULL,
    key TEXT NOT NULL, value TEXT NOT NULL,
    PRIMARY KEY (selection_id, source, sort_order)
  );

CREATE TABLE shaping_selected_offers (
  selection_id TEXT PRIMARY KEY, run_id TEXT, destination_slug TEXT, source_id TEXT, source_offer_id TEXT,
  selected_depart_date TEXT, selected_return_date TEXT, nights INTEGER, price_per_person_twd INTEGER,
  price_total_twd INTEGER, hotel_name TEXT, observed_by TEXT, observed_at TEXT, selected_by TEXT,
  selected_at TEXT, imported_at TEXT
);

CREATE TABLE shaping_tour_group_offer_notes (
    run_id TEXT NOT NULL, offer_id TEXT NOT NULL, sort_order INTEGER NOT NULL,
    key TEXT NOT NULL, value TEXT NOT NULL,
    PRIMARY KEY (run_id, offer_id, sort_order)
  );

CREATE TABLE shaping_tour_group_offers (
    run_id TEXT NOT NULL,
    offer_id TEXT NOT NULL,
    source_id TEXT NOT NULL,
    dest_region TEXT NOT NULL,
    depart_date TEXT NOT NULL,
    return_date TEXT NOT NULL,
    nights INTEGER NOT NULL,
    price_per_person_twd INTEGER NOT NULL,
    title TEXT NOT NULL,
    url TEXT NOT NULL,
    scraped_at TEXT NOT NULL,
    hotel_name TEXT,
    hotel_star_rating INTEGER,
    meals_included_count INTEGER,
    departure_status TEXT,
    seats_available INTEGER,
    min_group_size INTEGER,
    group_size_cap INTEGER,
    product_kind TEXT NOT NULL DEFAULT 'group_tour', parse_warnings_text TEXT, raw_confidence TEXT, raw_note TEXT, raw_flight TEXT, raw_flight_outbound TEXT, raw_flight_return TEXT,
    PRIMARY KEY (run_id, offer_id)
  );

CREATE TABLE shaping_tour_group_scrape_attempts (
    run_id TEXT NOT NULL,
    source_id TEXT NOT NULL,
    dest_region TEXT NOT NULL,
    nights INTEGER NOT NULL,
    status TEXT NOT NULL,
    offer_count INTEGER,
    parsed_count INTEGER,
    skipped_count INTEGER,
    error TEXT,
    attempted_at TEXT,
    PRIMARY KEY (run_id, source_id, dest_region, nights)
  );

CREATE TABLE "timesofday" (
    plan_id TEXT NOT NULL,
    destination TEXT NOT NULL,
    day_number INTEGER NOT NULL,
    session_type TEXT NOT NULL CHECK(session_type IN ('morning','noon','afternoon','evening')),
    focus TEXT,
    transit_notes TEXT,
    booking_notes TEXT,
    time_range_start TEXT,
    time_range_end TEXT,
    focus_zh TEXT,
    transit_notes_zh TEXT,
    updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (plan_id, destination, day_number, session_type)
  );

CREATE TABLE transport_extra_candidates (
    plan_id TEXT NOT NULL, destination TEXT NOT NULL,
    direction TEXT NOT NULL,  -- 'home_to_airport' | 'airport_to_hotel'
    sort_order INTEGER NOT NULL, candidate_id TEXT,
    method TEXT, route TEXT, departure_time TEXT, arrival_time TEXT,
    duration_min INTEGER, cost_jpy INTEGER, transfers INTEGER,
    extra_text TEXT,  -- joined amenities/pros/cons (open/variable fields)
    PRIMARY KEY (plan_id, destination, direction, sort_order)
  );

CREATE TABLE transport_hubs (
    region TEXT,
    hub_id TEXT,
    hub_type TEXT,
    area TEXT,
    source_url TEXT,
    fetched_at TEXT,
    confidence TEXT,
    PRIMARY KEY (region, hub_id)
  );

CREATE TABLE transport_routes (
    region TEXT,
    route_key TEXT,
    from_hub TEXT,
    to_hub TEXT,
    time_min INTEGER,
    cost_jpy INTEGER,
    method TEXT,
    source_url TEXT,
    fetched_at TEXT,
    confidence TEXT,
    PRIMARY KEY (region, route_key)
  );

CREATE TABLE transportation_extras (
  plan_id TEXT NOT NULL, destination TEXT NOT NULL,
  source TEXT, populated_from TEXT, research_notes TEXT,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP, home_to_airport_status TEXT, airport_to_hotel_status TEXT,
  PRIMARY KEY (plan_id, destination)
);

-- =====================
-- Indexes
-- =====================

CREATE INDEX idx_activities_booking ON activities(plan_id, booking_status);
CREATE INDEX idx_activities_session ON activities(plan_id, destination, day_number, session_type, sort_order);
CREATE INDEX idx_bc_dest ON bookings_current(destination, category);
CREATE INDEX idx_bc_offer ON bookings_current(offer_id);
CREATE INDEX idx_bc_status ON bookings_current(status);
CREATE INDEX idx_be_key ON bookings_events(booking_key, event_at);
CREATE INDEX idx_events_dest ON events(destination);
CREATE UNIQUE INDEX idx_events_external_id ON events(external_id);
CREATE INDEX idx_events_type ON events(event_type);
CREATE INDEX idx_holidays_country_date ON holidays(country, date);
CREATE INDEX idx_offers_date ON offers (departure_date);
CREATE UNIQUE INDEX idx_offers_dedup ON offers(id, scraped_at);
CREATE INDEX idx_offers_price ON offers (price_per_person);
CREATE INDEX idx_offers_region ON offers (region);
CREATE INDEX idx_offers_source ON offers (source_id);
CREATE UNIQUE INDEX idx_operation_runs_idempotency ON operation_runs(plan_id, idempotency_key);
CREATE INDEX idx_operation_runs_plan ON operation_runs(plan_id, started_at DESC);
CREATE INDEX idx_s0_cand_run ON shaping_candidates(run_id, rank);
CREATE INDEX idx_s0_shaping_run ON shaping_rules(run_id, aspect, role);
CREATE UNIQUE INDEX uq_s0_shaping_value
    ON shaping_rules(
      run_id, aspect, role, kind,
      COALESCE(value_text, ''),
      COALESCE(value_date, ''),
      COALESCE(value_integer, 0)
    );
CREATE INDEX idx_s0_tg_offers_lookup ON shaping_tour_group_offers(run_id, dest_region, nights, price_per_person_twd);
