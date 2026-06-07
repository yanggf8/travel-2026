import { TursoPipelineClient, tursoText, tursoInt } from './turso-pipeline';

async function main() {
  const client = new TursoPipelineClient();
  
  console.log('Running Turso schema migrations...');

  try {
    // 1. Add external_id to events
    console.log('Checking for external_id column in events table...');
    await client.execute('ALTER TABLE events ADD COLUMN external_id TEXT;');
    console.log('✅ Added external_id column.');
  } catch (e: any) {
    if (e.message?.includes('duplicate column name') || e.message?.includes('already exists')) {
      console.log('ℹ️  external_id column already exists.');
    } else {
      console.warn('⚠️  Could not add external_id column:', e.message);
    }
  }

  try {
    // 2. Add unique index on events.external_id
    console.log('Creating unique index on external_id...');
    await client.execute('CREATE UNIQUE INDEX idx_events_external_id ON events(external_id);');
    console.log('✅ Created unique index.');
  } catch (e: any) {
    if (e.message?.includes('already exists')) {
      console.log('ℹ️  Index already exists.');
    } else {
      console.warn('⚠️  Could not create index:', e.message);
    }
  }

  // 3. Add source_file column to offers
  try {
    console.log('Checking for source_file column in offers table...');
    await client.execute('ALTER TABLE offers ADD COLUMN source_file TEXT;');
    console.log('✅ Added source_file column.');
  } catch (e: any) {
    if (e.message?.includes('duplicate column name') || e.message?.includes('already exists')) {
      console.log('ℹ️  source_file column already exists.');
    } else {
      console.warn('⚠️  Could not add source_file column:', e.message);
    }
  }

  // 4. Add dedup index on offers(id, scraped_at) for append-only ingestion
  try {
    console.log('Creating dedup index on offers(id, scraped_at)...');
    await client.execute('CREATE UNIQUE INDEX IF NOT EXISTS idx_offers_dedup ON offers(id, scraped_at);');
    console.log('✅ Created dedup index.');
  } catch (e: any) {
    if (e.message?.includes('already exists')) {
      console.log('ℹ️  Dedup index already exists.');
    } else {
      console.warn('⚠️  Could not create dedup index:', e.message);
    }
  }

  // 5. Create bookings table for booking decision sync
  try {
    console.log('Creating bookings table...');
    await client.execute(`CREATE TABLE IF NOT EXISTS bookings (
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
);`);
    console.log('✅ Created bookings table.');
  } catch (e: any) {
    if (e.message?.includes('already exists')) {
      console.log('ℹ️  Bookings table already exists.');
    } else {
      console.warn('⚠️  Could not create bookings table:', e.message);
    }
  }

  // 6. Drop plan_snapshots table (was blob-based archival, replaced by operation_runs audit trail)
  try {
    console.log('Dropping plan_snapshots table (blob-based, obsolete)...');
    await client.execute('DROP TABLE IF EXISTS plan_snapshots;');
    console.log('✅ Dropped plan_snapshots table.');
  } catch (e: any) {
    console.warn('⚠️  Could not drop plan_snapshots:', e.message);
  }

  // 7. Create bookings_current table (flat queryable booking rows)
  try {
    console.log('Creating bookings_current table...');
    await client.execute(`CREATE TABLE IF NOT EXISTS bookings_current (
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
);`);
    await client.execute('CREATE INDEX IF NOT EXISTS idx_bc_dest ON bookings_current(destination, category);');
    await client.execute('CREATE INDEX IF NOT EXISTS idx_bc_status ON bookings_current(status);');
    await client.execute('CREATE INDEX IF NOT EXISTS idx_bc_offer ON bookings_current(offer_id);');
    console.log('✅ Created bookings_current table.');
  } catch (e: any) {
    if (e.message?.includes('already exists')) {
      console.log('ℹ️  bookings_current table already exists.');
    } else {
      console.warn('⚠️  Could not create bookings_current table:', e.message);
    }
  }

  // 8. Create bookings_events table (audit trail)
  try {
    console.log('Creating bookings_events table...');
    await client.execute(`CREATE TABLE IF NOT EXISTS bookings_events (
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
);`);
    await client.execute('CREATE INDEX IF NOT EXISTS idx_be_key ON bookings_events(booking_key, event_at);');
    console.log('✅ Created bookings_events table.');
  } catch (e: any) {
    if (e.message?.includes('already exists')) {
      console.log('ℹ️  bookings_events table already exists.');
    } else {
      console.warn('⚠️  Could not create bookings_events table:', e.message);
    }
  }

  // 9. Create plans table (DB-primary plan storage)
  // Skip if plans_current exists — step 13 will rename it to plans
  try {
    const legacyCheck = await client.execute("SELECT name FROM sqlite_master WHERE type='table' AND name='plans_current'");
    const legacyExists = (legacyCheck?.results?.[0]?.response?.result?.rows?.length ?? 0) > 0;
    if (legacyExists) {
      console.log('ℹ️  plans_current exists — step 13 will rename it to plans.');
    } else {
      console.log('Creating plans table...');
      await client.execute(`CREATE TABLE IF NOT EXISTS plans (
  plan_id TEXT PRIMARY KEY,
  schema_version TEXT NOT NULL,
  plan_json TEXT NOT NULL,
  state_json TEXT,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
);`);
      console.log('✅ Created plans table.');
    }
  } catch (e: any) {
    if (e.message?.includes('already exists')) {
      console.log('ℹ️  plans table already exists.');
    } else {
      console.warn('⚠️  Could not create plans table:', e.message);
    }
  }

  // 10. Create normalized itinerary tables (Phase 1)
  const itineraryTables: Array<{ name: string; sql: string }> = [
    {
      name: 'days',
      sql: `CREATE TABLE IF NOT EXISTS days (
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
  feels_like_low_c REAL,
  feels_like_high_c REAL,
  weather_source_id TEXT,
  weather_sourced_at TEXT,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination, day_number)
);`,
    },
    {
      name: 'timesofday',
      sql: `CREATE TABLE IF NOT EXISTS timesofday (
  plan_id TEXT NOT NULL,
  destination TEXT NOT NULL,
  day_number INTEGER NOT NULL,
  session_type TEXT NOT NULL CHECK(session_type IN ('morning', 'noon', 'afternoon', 'evening')),
  focus TEXT,
  transit_notes TEXT,
  booking_notes TEXT,
  time_range_start TEXT,
  time_range_end TEXT,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination, day_number, session_type)
);`,
    },
    {
      name: 'activities',
      sql: `CREATE TABLE IF NOT EXISTS activities (
  id TEXT PRIMARY KEY,
  plan_id TEXT NOT NULL,
  destination TEXT NOT NULL,
  day_number INTEGER NOT NULL,
  session_type TEXT NOT NULL CHECK(session_type IN ('morning', 'noon', 'afternoon', 'evening')),
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
  notes TEXT,
  priority TEXT NOT NULL DEFAULT 'want' CHECK(priority IN ('must', 'want', 'optional')),
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
);`,
    },
    {
      name: 'plan_metadata',
      sql: `CREATE TABLE IF NOT EXISTS plan_metadata (
  plan_id TEXT PRIMARY KEY,
  schema_version TEXT NOT NULL,
  active_destination TEXT,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
);`,
    },
    {
      name: 'date_anchors',
      sql: `CREATE TABLE IF NOT EXISTS date_anchors (
  plan_id TEXT NOT NULL,
  destination TEXT NOT NULL,
  start_date TEXT NOT NULL,
  end_date TEXT NOT NULL,
  days INTEGER NOT NULL,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination)
);`,
    },
    {
      name: 'process_statuses',
      sql: `CREATE TABLE IF NOT EXISTS process_statuses (
  plan_id TEXT NOT NULL,
  destination TEXT NOT NULL,
  process_id TEXT NOT NULL,
  status TEXT NOT NULL,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination, process_id)
);`,
    },
    {
      name: 'cascade_dirty_flags',
      sql: `CREATE TABLE IF NOT EXISTS cascade_dirty_flags (
  plan_id TEXT NOT NULL,
  destination TEXT NOT NULL,
  process_id TEXT NOT NULL,
  dirty INTEGER NOT NULL DEFAULT 0,
  last_changed DATETIME,
  PRIMARY KEY (plan_id, destination, process_id)
);`,
    },
    {
      name: 'airport_transfers',
      sql: `CREATE TABLE IF NOT EXISTS airport_transfers (
  plan_id TEXT NOT NULL,
  destination TEXT NOT NULL,
  direction TEXT NOT NULL CHECK(direction IN ('arrival', 'departure')),
  status TEXT NOT NULL DEFAULT 'planned' CHECK(status IN ('planned', 'booked')),
  selected_json TEXT,
  candidates_json TEXT,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination, direction)
);`,
    },
    {
      name: 'flights',
      sql: `CREATE TABLE IF NOT EXISTS flights (
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
);`,
    },
    {
      name: 'hotels',
      sql: `CREATE TABLE IF NOT EXISTS hotels (
  plan_id TEXT NOT NULL,
  destination TEXT NOT NULL,
  populated_from TEXT,
  name TEXT,
  access_json TEXT,
  check_in TEXT,
  notes TEXT,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination)
);`,
    },
  ];

  for (const table of itineraryTables) {
    try {
      console.log(`Creating ${table.name} table...`);
      await client.execute(table.sql);
      console.log(`✅ Created ${table.name} table.`);
    } catch (e: any) {
      if (e.message?.includes('already exists')) {
        console.log(`ℹ️  ${table.name} table already exists.`);
      } else {
        console.warn(`⚠️  Could not create ${table.name} table:`, e.message);
      }
    }
  }

  // Indexes for normalized tables
  const indexes = [
    'CREATE INDEX IF NOT EXISTS idx_activities_session ON activities(plan_id, destination, day_number, session_type, sort_order)',
    'CREATE INDEX IF NOT EXISTS idx_activities_booking ON activities(plan_id, booking_status)',
  ];
  for (const idx of indexes) {
    try {
      await client.execute(idx);
    } catch (e: any) {
      if (!e.message?.includes('already exists')) {
        console.warn(`⚠️  Index creation warning:`, e.message);
      }
    }
  }
  console.log('✅ Created normalized table indexes.');

  // 11. Add version column to plans table (whichever name currently exists)
  try {
    const plansCheck = await client.execute("SELECT name FROM sqlite_master WHERE type='table' AND name IN ('plans', 'plans_current') ORDER BY name");
    const plansTableName = (plansCheck?.results?.[0]?.response?.result?.rows?.[0] as any)?.[0]?.value || 'plans';
    console.log(`Adding version column to ${plansTableName}...`);
    await client.execute(`ALTER TABLE ${plansTableName} ADD COLUMN version INTEGER NOT NULL DEFAULT 0;`);
    console.log('✅ Added version column.');
  } catch (e: any) {
    if (e.message?.includes('duplicate column name') || e.message?.includes('already exists')) {
      console.log('ℹ️  version column already exists.');
    } else {
      console.warn('⚠️  Could not add version column:', e.message);
    }
  }

  // 12. Create operation_runs table (operation audit trail)
  try {
    console.log('Creating operation_runs table...');
    await client.execute(`CREATE TABLE IF NOT EXISTS operation_runs (
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
);`);
    await client.execute('CREATE INDEX IF NOT EXISTS idx_operation_runs_plan ON operation_runs(plan_id, started_at DESC);');
    await client.execute('CREATE UNIQUE INDEX IF NOT EXISTS idx_operation_runs_idempotency ON operation_runs(plan_id, idempotency_key);');
    console.log('✅ Created operation_runs table.');
  } catch (e: any) {
    if (e.message?.includes('already exists')) {
      console.log('ℹ️  operation_runs table already exists.');
    } else {
      console.warn('⚠️  Could not create operation_runs table:', e.message);
    }
  }

  // 13. Rename plans_current → plans (clearer name for multi-plan table)
  try {
    const oldExists = await client.execute("SELECT name FROM sqlite_master WHERE type='table' AND name='plans_current'");
    const oldHasRows = (oldExists?.results?.[0]?.response?.result?.rows?.length ?? 0) > 0;
    if (oldHasRows) {
      console.log('Renaming plans_current → plans...');
      await client.execute('ALTER TABLE plans_current RENAME TO plans');
      console.log('✅ Renamed plans_current → plans.');
    } else {
      const newExists = await client.execute("SELECT name FROM sqlite_master WHERE type='table' AND name='plans'");
      const newHasRows = (newExists?.results?.[0]?.response?.result?.rows?.length ?? 0) > 0;
      if (newHasRows) {
        console.log('ℹ️  Table already named "plans".');
      } else {
        console.warn('⚠️  Neither plans_current nor plans table found.');
      }
    }
  } catch (e: any) {
    console.warn('⚠️  Could not rename table:', e.message);
  }

  // 14. Add feels_like columns to days
  for (const col of ['feels_like_low_c', 'feels_like_high_c']) {
    try {
      console.log(`Adding ${col} column to days...`);
      await client.execute(`ALTER TABLE days ADD COLUMN ${col} REAL;`);
      console.log(`✅ Added ${col} column.`);
    } catch (e: any) {
      if (e.message?.includes('duplicate column name') || e.message?.includes('already exists')) {
        console.log(`ℹ️  ${col} column already exists.`);
      } else {
        console.warn(`⚠️  Could not add ${col} column:`, e.message);
      }
    }
  }

  // 15. Create flight_legs table (normalized — replaces JSON blobs in flights table)
  try {
    console.log('Creating flight_legs table...');
    await client.execute(`CREATE TABLE IF NOT EXISTS flight_legs (
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
);`);
    console.log('✅ Created flight_legs table.');
  } catch (e: any) {
    if (e.message?.includes('already exists')) {
      console.log('ℹ️  flight_legs table already exists.');
    } else {
      console.warn('⚠️  Could not create flight_legs table:', e.message);
    }
  }

  // 15b. Migrate existing flights rows → flight_legs (one-time, idempotent)
  try {
    console.log('Migrating flights → flight_legs...');
    const existingLegs = await client.execute(`SELECT COUNT(*) as cnt FROM flight_legs`);
    const legCount = parseInt((existingLegs?.results?.[0]?.response?.result?.rows?.[0] as any)?.[0]?.value || '0', 10);

    if (legCount > 0) {
      console.log(`ℹ️  flight_legs already has ${legCount} rows, skipping migration.`);
    } else {
      const flightRows = await client.execute(`SELECT plan_id, destination, populated_from, airline, airline_code, outbound_json, return_json, booked_date FROM flights`);
      const result = flightRows?.results?.[0]?.response?.result as any;
      const rawRows = result?.rows ?? [];
      const colNames = (result?.cols ?? []).map((c: any) => c.name);
      if (rawRows.length > 0 && colNames.length > 0) {
        for (const row of rawRows) {
          const obj: Record<string, string | null> = {};
          colNames.forEach((name: string, i: number) => {
            obj[name] = (row as any)[i]?.value ?? null;
          });

          for (const dir of ['outbound', 'return'] as const) {
            const jsonCol = dir === 'outbound' ? 'outbound_json' : 'return_json';
            const jsonStr = obj[jsonCol];
            if (!jsonStr) continue;
            try {
              const leg = JSON.parse(jsonStr);
              const esc = (s: string | null | undefined) => s ? `'${s.replace(/'/g, "''")}'` : 'NULL';
              await client.execute(`INSERT OR IGNORE INTO flight_legs
                (plan_id, destination, direction, leg_order, flight_number, airline, airline_code,
                 departure_airport, departure_code, departure_terminal, departure_time,
                 arrival_airport, arrival_code, arrival_terminal, arrival_time,
                 flight_date, populated_from, booked_date, updated_at)
                VALUES (${esc(obj.plan_id)}, ${esc(obj.destination)}, '${dir}', 0,
                 ${esc(leg.flight_number)}, ${esc(obj.airline)}, ${esc(obj.airline_code)},
                 ${esc(leg.departure_airport)}, ${esc(leg.departure_airport_code)}, ${esc(leg.departure_terminal)}, ${esc(leg.departure_time)},
                 ${esc(leg.arrival_airport)}, ${esc(leg.arrival_airport_code)}, ${esc(leg.arrival_terminal)}, ${esc(leg.arrival_time)},
                 ${esc(leg.date)}, ${esc(obj.populated_from)}, ${esc(obj.booked_date)}, datetime('now'))`);
            } catch { /* ignore parse errors */ }
          }
        }
        console.log('✅ Migrated flights → flight_legs.');
      } else {
        console.log('ℹ️  No flights rows to migrate.');
      }
    }
  } catch (e: any) {
    console.warn('⚠️  Could not migrate flights → flight_legs:', e.message);
  }

  // ========================================
  // Phase 1: Full Normalization Tables (v5.0.0)
  // ========================================

  const phase1Tables: Array<{ name: string; sql: string; indexes?: string[] }> = [
    {
      name: 'plan_destinations',
      sql: `CREATE TABLE IF NOT EXISTS plan_destinations (
  plan_id TEXT NOT NULL, slug TEXT NOT NULL, display_name TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'draft',
  created_at DATETIME, updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, slug)
)`,
    },
    {
      name: 'destination_details',
      sql: `CREATE TABLE IF NOT EXISTS destination_details (
  plan_id TEXT NOT NULL, destination TEXT NOT NULL,
  origin_city TEXT, region TEXT, primary_airport TEXT,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination)
)`,
    },
    {
      name: 'destination_cities',
      sql: `CREATE TABLE IF NOT EXISTS destination_cities (
  plan_id TEXT NOT NULL, destination TEXT NOT NULL, city_slug TEXT NOT NULL,
  role TEXT NOT NULL, nights INTEGER,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination, city_slug)
)`,
    },
    {
      name: 'plan_offers',
      sql: `CREATE TABLE IF NOT EXISTS plan_offers (
  plan_id TEXT NOT NULL, destination TEXT NOT NULL, id TEXT NOT NULL,
  source_id TEXT NOT NULL, type TEXT NOT NULL, title TEXT,
  price_per_person INTEGER, currency TEXT DEFAULT 'TWD', availability TEXT,
  url TEXT, scraped_at TEXT, product_code TEXT, duration_days INTEGER,
  price_total INTEGER, seats_remaining INTEGER, includes_json TEXT,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination, id)
)`,
    },
    {
      name: 'plan_offer_flights',
      sql: `CREATE TABLE IF NOT EXISTS plan_offer_flights (
  plan_id TEXT NOT NULL, destination TEXT NOT NULL, offer_id TEXT NOT NULL,
  direction TEXT NOT NULL CHECK(direction IN ('outbound', 'return')),
  flight_number TEXT, airline TEXT, airline_code TEXT,
  departure_code TEXT, departure_time TEXT, arrival_code TEXT, arrival_time TEXT,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination, offer_id, direction)
)`,
    },
    {
      name: 'plan_offer_hotels',
      sql: `CREATE TABLE IF NOT EXISTS plan_offer_hotels (
  plan_id TEXT NOT NULL, destination TEXT NOT NULL, offer_id TEXT NOT NULL,
  name TEXT, slug TEXT, area TEXT, star_rating INTEGER, access_json TEXT,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination, offer_id)
)`,
    },
    {
      name: 'plan_offer_date_pricing',
      sql: `CREATE TABLE IF NOT EXISTS plan_offer_date_pricing (
  plan_id TEXT NOT NULL, destination TEXT NOT NULL, offer_id TEXT NOT NULL,
  date TEXT NOT NULL, price INTEGER NOT NULL, availability TEXT, seats_remaining INTEGER,
  currency TEXT DEFAULT 'TWD',
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination, offer_id, date)
)`,
    },
    {
      name: 'plan_offer_best_value',
      sql: `CREATE TABLE IF NOT EXISTS plan_offer_best_value (
  plan_id TEXT NOT NULL, destination TEXT NOT NULL, offer_id TEXT NOT NULL,
  best_date TEXT, best_price INTEGER, currency TEXT DEFAULT 'TWD',
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination, offer_id)
)`,
    },
    {
      name: 'plan_offer_selection',
      sql: `CREATE TABLE IF NOT EXISTS plan_offer_selection (
  plan_id TEXT NOT NULL, destination TEXT NOT NULL,
  selected_offer_id TEXT, selected_date TEXT, selected_at DATETIME,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination)
)`,
    },
    {
      name: 'plan_offer_provenance',
      sql: `CREATE TABLE IF NOT EXISTS plan_offer_provenance (
  plan_id TEXT NOT NULL, destination TEXT NOT NULL,
  source_id TEXT NOT NULL, scraped_at TEXT NOT NULL,
  file_path TEXT,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination, source_id, scraped_at)
)`,
    },
    {
      name: 'plan_offer_warnings',
      sql: `CREATE TABLE IF NOT EXISTS plan_offer_warnings (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  plan_id TEXT NOT NULL, destination TEXT NOT NULL, offer_id TEXT,
  warning_type TEXT NOT NULL, message TEXT NOT NULL,
  created_at DATETIME DEFAULT CURRENT_TIMESTAMP
)`,
    },
    {
      name: 'plan_budget',
      sql: `CREATE TABLE IF NOT EXISTS plan_budget (
  plan_id TEXT PRIMARY KEY,
  total_cap INTEGER, flight_cap INTEGER, accommodation_cap INTEGER, daily_cap INTEGER,
  pax INTEGER NOT NULL DEFAULT 1, currency TEXT DEFAULT 'TWD',
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
)`,
    },
    {
      name: 'cascade_triggers',
      sql: `CREATE TABLE IF NOT EXISTS cascade_triggers (
  plan_id TEXT NOT NULL, trigger_id TEXT NOT NULL,
  event TEXT NOT NULL, reset_json TEXT NOT NULL, scope TEXT NOT NULL,
  condition_json TEXT, action TEXT, populate_map_json TEXT, set_source TEXT,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, trigger_id)
)`,
    },
    {
      name: 'plan_schema_contract',
      sql: `CREATE TABLE IF NOT EXISTS plan_schema_contract (
  plan_id TEXT PRIMARY KEY,
  id_convention TEXT NOT NULL, currency TEXT NOT NULL DEFAULT 'TWD',
  process_nodes_json TEXT NOT NULL,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
)`,
    },
    {
      name: 'plan_process_precedence',
      sql: `CREATE TABLE IF NOT EXISTS plan_process_precedence (
  plan_id TEXT PRIMARY KEY,
  precedence_json TEXT NOT NULL,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
)`,
    },
    {
      name: 'cascade_global_state',
      sql: `CREATE TABLE IF NOT EXISTS cascade_global_state (
  plan_id TEXT PRIMARY KEY,
  last_cascade_run DATETIME, active_dest_last TEXT, p1_dirty INTEGER NOT NULL DEFAULT 0,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
)`,
    },
    {
      name: 'plan_root_date_anchor',
      sql: `CREATE TABLE IF NOT EXISTS plan_root_date_anchor (
  plan_id TEXT PRIMARY KEY,
  status TEXT NOT NULL, set_out_date TEXT, duration_days INTEGER, return_date TEXT,
  flexibility_json TEXT,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
)`,
    },
    {
      name: 'itinerary_metadata',
      sql: `CREATE TABLE IF NOT EXISTS itinerary_metadata (
  plan_id TEXT NOT NULL, destination TEXT NOT NULL,
  scaffolded_at DATETIME, populated_at DATETIME, transit_summary TEXT,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination)
)`,
    },
    {
      name: 'accommodation_location_zone',
      sql: `CREATE TABLE IF NOT EXISTS accommodation_location_zone (
  plan_id TEXT NOT NULL, destination TEXT NOT NULL,
  selected_area TEXT, source TEXT, candidates_json TEXT,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination)
)`,
    },
    {
      name: 'transportation_extras',
      sql: `CREATE TABLE IF NOT EXISTS transportation_extras (
  plan_id TEXT NOT NULL, destination TEXT NOT NULL,
  source TEXT, populated_from TEXT, research_notes TEXT,
  home_to_airport_json TEXT, airport_to_hotel_json TEXT,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination)
)`,
    },
    {
      name: 'event_log_state',
      sql: `CREATE TABLE IF NOT EXISTS event_log_state (
  plan_id TEXT PRIMARY KEY,
  session TEXT NOT NULL, project TEXT NOT NULL, version TEXT NOT NULL,
  current_focus TEXT, active_destination TEXT, next_actions_json TEXT,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
)`,
    },
    {
      name: 'event_log_global_processes',
      sql: `CREATE TABLE IF NOT EXISTS event_log_global_processes (
  plan_id TEXT NOT NULL, process_id TEXT NOT NULL,
  status TEXT NOT NULL, events_json TEXT,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, process_id)
)`,
    },
    {
      name: 'event_log_destinations',
      sql: `CREATE TABLE IF NOT EXISTS event_log_destinations (
  plan_id TEXT NOT NULL, destination TEXT NOT NULL,
  status TEXT NOT NULL,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination)
)`,
    },
    {
      name: 'event_log_dest_processes',
      sql: `CREATE TABLE IF NOT EXISTS event_log_dest_processes (
  plan_id TEXT NOT NULL, destination TEXT NOT NULL, process_id TEXT NOT NULL,
  status TEXT NOT NULL, events_json TEXT,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination, process_id)
)`,
    },
    {
      name: 'event_log_process_events',
      sql: `CREATE TABLE IF NOT EXISTS event_log_process_events (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  plan_id TEXT NOT NULL, destination TEXT, process_id TEXT NOT NULL,
  event_type TEXT NOT NULL, event_data TEXT,
  event_at DATETIME DEFAULT CURRENT_TIMESTAMP
)`,
    },
    {
      name: 'airport_transfer_candidates',
      sql: `CREATE TABLE IF NOT EXISTS airport_transfer_candidates (
  plan_id TEXT NOT NULL, destination TEXT NOT NULL,
  direction TEXT NOT NULL CHECK(direction IN ('arrival', 'departure')),
  candidate_id TEXT NOT NULL,
  title TEXT NOT NULL, route TEXT, duration_min INTEGER, price_yen INTEGER,
  schedule TEXT, booking_url TEXT, notes TEXT, sort_order INTEGER NOT NULL DEFAULT 0,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination, direction, candidate_id)
)`,
    },
    {
      name: 'hotel_access_lines',
      sql: `CREATE TABLE IF NOT EXISTS hotel_access_lines (
  plan_id TEXT NOT NULL, destination TEXT NOT NULL,
  sort_order INTEGER NOT NULL, line TEXT NOT NULL,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination, sort_order)
)`,
    },
    {
      name: 'session_meals',
      sql: `CREATE TABLE IF NOT EXISTS session_meals (
  plan_id TEXT NOT NULL, destination TEXT NOT NULL,
  day_number INTEGER NOT NULL, session_type TEXT NOT NULL,
  sort_order INTEGER NOT NULL, meal TEXT NOT NULL,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination, day_number, session_type, sort_order)
)`,
    },
    {
      name: 'activity_tags',
      sql: `CREATE TABLE IF NOT EXISTS activity_tags (
  activity_id TEXT NOT NULL, tag TEXT NOT NULL,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (activity_id, tag)
)`,
    },
  ];

  for (const table of phase1Tables) {
    try {
      console.log(`Creating ${table.name} table...`);
      await client.execute(table.sql);
      console.log(`✅ Created ${table.name} table.`);
    } catch (e: any) {
      if (e.message?.includes('already exists')) {
        console.log(`ℹ️  ${table.name} table already exists.`);
      } else {
        console.warn(`⚠️  Could not create ${table.name} table:`, e.message);
      }
    }
  }

  // Add selected_* scalar columns to airport_transfers
  for (const col of [
    'selected_title TEXT', 'selected_route TEXT', 'selected_duration_min INTEGER',
    'selected_price_yen INTEGER', 'selected_schedule TEXT', 'selected_booking_url TEXT', 'selected_notes TEXT',
  ]) {
    try {
      await client.execute(`ALTER TABLE airport_transfers ADD COLUMN ${col};`);
      console.log(`✅ Added ${col.split(' ')[0]} to airport_transfers.`);
    } catch (e: any) {
      if (e.message?.includes('duplicate column') || e.message?.includes('already exists')) {
        console.log(`ℹ️  ${col.split(' ')[0]} already exists.`);
      } else {
        console.warn(`⚠️  Could not add ${col.split(' ')[0]}:`, e.message);
      }
    }
  }

  console.log('✅ Phase 1 normalization tables created.');

  // Phase 6: Drop blob columns (plan_json, state_json) from plans table.
  // All data is now in normalized tables. SQLite 3.35+ / Turso supports DROP COLUMN.
  for (const col of ['plan_json', 'state_json']) {
    try {
      console.log(`Dropping ${col} column from plans...`);
      await client.execute(`ALTER TABLE plans DROP COLUMN ${col};`);
      console.log(`✅ Dropped ${col} from plans.`);
    } catch (e: any) {
      if (e.message?.includes('no such column') || e.message?.includes('not found')) {
        console.log(`ℹ️  ${col} already dropped.`);
      } else {
        console.warn(`⚠️  Could not drop ${col}:`, e.message);
      }
    }
  }

  // ========================================
  // ZH Content Migration: ALTER + CREATE
  // ========================================

  const zhAlters = [
    { table: 'days', col: 'theme_zh TEXT' },
    { table: 'timesofday', col: 'focus_zh TEXT' },
    { table: 'timesofday', col: 'transit_notes_zh TEXT' },
    // meals_zh_json / activities_zh_json removed — Batch C de-JSON:
    // activities_zh → session_activities_zh; meals_zh_json was dead transit text.
    { table: 'hotels', col: 'name_zh TEXT' },
    { table: 'itinerary_metadata', col: 'transit_summary_zh TEXT' },
    { table: 'plan_destinations', col: 'home_address TEXT' },
  ];
  for (const { table, col } of zhAlters) {
    try {
      await client.execute(`ALTER TABLE ${table} ADD COLUMN ${col};`);
      console.log(`✅ Added ${col.split(' ')[0]} to ${table}.`);
    } catch (e: any) {
      if (e.message?.includes('duplicate column') || e.message?.includes('already exists')) {
        console.log(`ℹ️  ${col.split(' ')[0]} already exists on ${table}.`);
      } else {
        console.warn(`⚠️  Could not add ${col.split(' ')[0]} to ${table}:`, e.message);
      }
    }
  }

  try {
    await client.execute(`CREATE TABLE IF NOT EXISTS day_route_segments (
  plan_id TEXT NOT NULL, destination TEXT NOT NULL,
  day_number INTEGER NOT NULL, sort_order INTEGER NOT NULL,
  from_place TEXT NOT NULL, to_place TEXT NOT NULL,
  mode TEXT NOT NULL,
  PRIMARY KEY (plan_id, destination, day_number, sort_order)
)`);
    console.log('✅ Created day_route_segments table.');
  } catch (e: any) {
    if (e.message?.includes('already exists')) console.log('ℹ️  day_route_segments already exists.');
    else console.warn('⚠️  Could not create day_route_segments:', e.message);
  }

  try {
    await client.execute(`CREATE TABLE IF NOT EXISTS day_landmarks (
  plan_id TEXT NOT NULL, destination TEXT NOT NULL,
  day_number INTEGER NOT NULL, sort_order INTEGER NOT NULL,
  landmark TEXT NOT NULL,
  PRIMARY KEY (plan_id, destination, day_number, sort_order)
)`);
    console.log('✅ Created day_landmarks table.');
  } catch (e: any) {
    if (e.message?.includes('already exists')) console.log('ℹ️  day_landmarks already exists.');
    else console.warn('⚠️  Could not create day_landmarks:', e.message);
  }

  console.log('✅ ZH content schema migration complete.');

  // Add display_name column to destination_cities
  try {
    await client.execute('ALTER TABLE destination_cities ADD COLUMN display_name TEXT;');
    console.log('✅ Added display_name to destination_cities.');
  } catch (e: any) {
    if (!e.message?.includes('duplicate column name')) throw e;
    console.log('ℹ️  display_name column already exists in destination_cities.');
  }

  // Add offer_count column to plan_offer_provenance
  try {
    await client.execute('ALTER TABLE plan_offer_provenance ADD COLUMN offer_count INTEGER;');
    console.log('✅ Added offer_count to plan_offer_provenance.');
  } catch (e: any) {
    if (e.message?.includes('duplicate column name') || e.message?.includes('already exists')) {
      console.log('ℹ️  offer_count column already exists in plan_offer_provenance.');
    } else {
      console.warn('⚠️  Could not add offer_count to plan_offer_provenance:', e.message);
    }
  }

  // ========================================
  // Global Config Tables: destination_config, origin_config, global_config
  // ========================================

  try {
    console.log('Creating destination_config table...');
    await client.execute(`CREATE TABLE IF NOT EXISTS destination_config (
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
)`);
    console.log('✅ Created destination_config table.');
  } catch (e: any) {
    if (e.message?.includes('already exists')) {
      console.log('ℹ️  destination_config table already exists.');
    } else {
      console.warn('⚠️  Could not create destination_config table:', e.message);
    }
  }

  try {
    console.log('Creating origin_config table...');
    await client.execute(`CREATE TABLE IF NOT EXISTS origin_config (
  slug TEXT PRIMARY KEY,
  country_code TEXT,
  currency TEXT,
  timezone TEXT,
  holiday_calendar TEXT,
  primary_airports_json TEXT,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
)`);
    console.log('✅ Created origin_config table.');
  } catch (e: any) {
    if (e.message?.includes('already exists')) {
      console.log('ℹ️  origin_config table already exists.');
    } else {
      console.warn('⚠️  Could not create origin_config table:', e.message);
    }
  }

  try {
    console.log('Creating global_config table...');
    await client.execute(`CREATE TABLE IF NOT EXISTS global_config (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
)`);
    console.log('✅ Created global_config table.');
  } catch (e: any) {
    if (e.message?.includes('already exists')) {
      console.log('ℹ️  global_config table already exists.');
    } else {
      console.warn('⚠️  Could not create global_config table:', e.message);
    }
  }

  // Child tables for destination/origin airports + markets must exist before the
  // config backfill writes to them (de-JSON: no JSON columns). Idempotent.
  await client.execute(`CREATE TABLE IF NOT EXISTS destination_airports (
    slug TEXT, airport TEXT, sort_order INTEGER, PRIMARY KEY (slug, sort_order)
  );`);
  await client.execute(`CREATE TABLE IF NOT EXISTS destination_markets (
    slug TEXT, market TEXT, sort_order INTEGER, PRIMARY KEY (slug, sort_order)
  );`);
  await client.execute(`CREATE TABLE IF NOT EXISTS origin_airports (
    slug TEXT, airport TEXT, sort_order INTEGER, PRIMARY KEY (slug, sort_order)
  );`);

  // Backfill destination_config rows. Airports/markets are scalar string arrays
  // here (source code, not DB JSON) and written to child tables below.
  const destinations = [
    {
      slug: 'tokyo_2026', display_name: 'Tokyo', ref_id: 'tokyo',
      ref_path: '',
      timezone: 'Asia/Tokyo', currency: 'JPY',
      markets: ['TW', 'JP'], primary_airports: ['NRT', 'HND'],
      language: 'ja', origin: 'taiwan', lat: 35.6762, lon: 139.6503,
    },
    {
      slug: 'nagoya_2026', display_name: 'Nagoya', ref_id: 'nagoya',
      ref_path: '',
      timezone: 'Asia/Tokyo', currency: 'JPY',
      markets: ['TW', 'JP'], primary_airports: ['NGO'],
      language: 'ja', origin: 'taiwan', lat: 35.1815, lon: 136.9066,
    },
    {
      slug: 'osaka_2026', display_name: 'Osaka', ref_id: 'osaka',
      ref_path: '',
      timezone: 'Asia/Tokyo', currency: 'JPY',
      markets: ['TW', 'JP'], primary_airports: ['KIX', 'ITM'],
      language: 'ja', origin: 'taiwan', lat: 34.6937, lon: 135.5023,
    },
    {
      slug: 'osaka_kyoto_2026', display_name: 'Osaka + Kyoto', ref_id: 'osaka_kyoto',
      ref_path: '',
      timezone: 'Asia/Tokyo', currency: 'JPY',
      markets: ['TW', 'JP'], primary_airports: ['KIX', 'ITM'],
      language: 'ja', origin: 'taiwan', lat: 34.6937, lon: 135.5023,
    },
    {
      slug: 'kyoto_2026', display_name: 'Kyoto', ref_id: 'kyoto',
      ref_path: '',
      timezone: 'Asia/Tokyo', currency: 'JPY',
      markets: ['TW', 'JP'], primary_airports: ['KIX'],
      language: 'ja', origin: 'taiwan', lat: 35.0116, lon: 135.7681,
    },
    {
      slug: 'okinawa_2026', display_name: 'Okinawa', ref_id: 'okinawa',
      ref_path: '',
      timezone: 'Asia/Tokyo', currency: 'JPY',
      markets: ['TW', 'JP'], primary_airports: ['OKA'],
      language: 'ja', origin: 'taiwan', lat: 26.2124, lon: 127.6792,
    },
  ];

  for (const d of destinations) {
    try {
      const esc = (s: string | null | undefined) => s !== null && s !== undefined ? `'${String(s).replace(/'/g, "''")}'` : 'NULL';
      await client.execute(
        `INSERT OR IGNORE INTO destination_config (slug, display_name, ref_id, ref_path, timezone, currency, language, origin, lat, lon) VALUES (${esc(d.slug)}, ${esc(d.display_name)}, ${esc(d.ref_id)}, ${esc(d.ref_path)}, ${esc(d.timezone)}, ${esc(d.currency)}, ${esc(d.language)}, ${esc(d.origin)}, ${d.lat}, ${d.lon})`
      );
      // Child rows (no JSON columns).
      await client.execute(`DELETE FROM destination_airports WHERE slug = ${esc(d.slug)}`);
      for (let i = 0; i < d.primary_airports.length; i++) {
        await client.execute(`INSERT INTO destination_airports (slug, airport, sort_order) VALUES (${esc(d.slug)}, ${esc(d.primary_airports[i])}, ${i})`);
      }
      await client.execute(`DELETE FROM destination_markets WHERE slug = ${esc(d.slug)}`);
      for (let i = 0; i < d.markets.length; i++) {
        await client.execute(`INSERT INTO destination_markets (slug, market, sort_order) VALUES (${esc(d.slug)}, ${esc(d.markets[i])}, ${i})`);
      }
      console.log(`✅ Backfilled destination_config: ${d.slug}`);
    } catch (e: any) {
      console.warn(`⚠️  Could not backfill destination_config ${d.slug}:`, e.message);
    }
  }
  await client.execute(`UPDATE destination_config SET ref_path = '' WHERE slug = 'kyoto_2026' AND ref_path = 'src/skills/travel-shared/references/destinations/kyoto.json';`);

  // Backfill origin_config rows
  try {
    await client.execute(
      `INSERT OR IGNORE INTO origin_config (slug, country_code, currency, timezone, holiday_calendar) VALUES ('taiwan', 'TW', 'TWD', 'Asia/Taipei', NULL)`
    );
    await client.execute(
      `UPDATE origin_config SET holiday_calendar = NULL WHERE slug = 'taiwan' AND holiday_calendar LIKE 'data/holidays/%'`
    );
    const taiwanAirports = ['TPE', 'TSA', 'RMQ', 'KHH'];
    await client.execute(`DELETE FROM origin_airports WHERE slug = 'taiwan'`);
    for (let i = 0; i < taiwanAirports.length; i++) {
      await client.execute(`INSERT INTO origin_airports (slug, airport, sort_order) VALUES ('taiwan', '${taiwanAirports[i]}', ${i})`);
    }
    console.log('✅ Backfilled origin_config: taiwan');
  } catch (e: any) {
    console.warn('⚠️  Could not backfill origin_config taiwan:', e.message);
  }

  // Backfill global_config rows
  const globalConfigs = [
    { key: 'default_destination', value: 'tokyo_2026' },
    { key: 'default_origin', value: 'taiwan' },
  ];
  for (const g of globalConfigs) {
    try {
      await client.execute(
        `INSERT OR IGNORE INTO global_config (key, value) VALUES ('${g.key}', '${g.value}')`
      );
      console.log(`✅ Backfilled global_config: ${g.key}=${g.value}`);
    } catch (e: any) {
      console.warn(`⚠️  Could not backfill global_config ${g.key}:`, e.message);
    }
  }

  // ========================================
  // OTA Sources: Create table + seed rows
  // ========================================

  try {
    console.log('Creating ota_sources table...');
    await client.execute(`CREATE TABLE IF NOT EXISTS ota_sources (
  source_id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  type_json TEXT,
  status TEXT DEFAULT 'active',
  scraper_script TEXT,
  regions_json TEXT,
  url_template TEXT,
  notes TEXT,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
)`);
    // Normalized list child tables (no JSON columns). type_json/regions_json
    // above stay only so a pre-migration DB upgrades cleanly; new reads/writes
    // use these child tables, and the JSON columns are dropped post-cutover.
    await client.execute(`CREATE TABLE IF NOT EXISTS ota_source_types (
      source_id TEXT, type TEXT, PRIMARY KEY (source_id, type)
    );`);
    await client.execute(`CREATE TABLE IF NOT EXISTS ota_source_regions (
      source_id TEXT, region TEXT, PRIMARY KEY (source_id, region)
    );`);
    console.log('✅ Created ota_sources table.');
  } catch (e: any) {
    if (e.message?.includes('already exists')) {
      console.log('ℹ️  ota_sources table already exists.');
    } else {
      console.warn('⚠️  Could not create ota_sources table:', e.message);
    }
  }

  const otaSources: Array<{
    source_id: string;
    name: string;
    type_json: string;
    status: string;
    scraper_script: string | null;
    regions_json: string | null;
    url_template: string | null;
    notes: string | null;
  }> = [
    {
      source_id: 'besttour',
      name: '喜鴻假期',
      type_json: '["package"]',
      status: 'active',
      scraper_script: 'scripts/scrape_package.py',
      regions_json: '["tokyo","kansai","hokkaido","kyushu","okinawa"]',
      url_template: 'https://www.besttour.com.tw',
      notes: 'Use listing URL for search, NOT /e_web/DOM/ (returns 404). Primarily group tours.',
    },
    {
      source_id: 'liontravel',
      name: '雄獅旅遊',
      type_json: '["package","flight","hotel"]',
      status: 'active',
      scraper_script: 'scripts/scrape_liontravel_dated.py',
      regions_json: '["tokyo","kansai","hokkaido","okinawa"]',
      url_template: 'https://vacation.liontravel.com/search?Destination={dest_code}&FromDate={from_yyyymmdd}&ToDate={to_yyyymmdd}&Days={days}&roomlist={adults}-0-0',
      notes: 'FIT search via vacation.liontravel.com, group tour URL unknown',
    },
    {
      source_id: 'tigerair',
      name: '台灣虎航',
      type_json: '["flight"]',
      status: 'active',
      scraper_script: 'scripts/scrape_tigerair.py',
      regions_json: null,
      url_template: 'https://www.tigerairtw.com',
      notes: 'Form-based scraper (no URL deep-linking), requires Playwright form interaction. No listing page.',
    },
    {
      source_id: 'lifetour',
      name: '五福旅遊',
      type_json: '["package","flight","hotel"]',
      status: 'active',
      scraper_script: 'scripts/scrape_package.py',
      regions_json: '["tokyo","kansai","hokkaido","kyushu","okinawa"]',
      url_template: 'https://tour.lifetour.com.tw/searchlist/{departure}/{region_code}',
      notes: 'Group tour listing at tour.lifetour.com.tw/searchlist/{departure}/{region}. Some packages include 伴自由 (semi-FIT) days.',
    },
    {
      source_id: 'settour',
      name: '東南旅遊',
      type_json: '["package","flight","hotel"]',
      status: 'active',
      scraper_script: 'scripts/scrape_package.py',
      regions_json: '["tokyo","kansai","hokkaido","kyushu","okinawa"]',
      url_template: 'https://tour.settour.com.tw/search?destinationCode={dest_code}',
      notes: 'Group tour uses .product-item containers with code in slider-flightInfo_* IDs. FIT available at fit.settour.com.tw.',
    },
    {
      source_id: 'travel4u',
      name: '山富旅遊',
      type_json: '["package"]',
      status: 'active',
      scraper_script: 'scripts/scrape_package.py',
      regions_json: '["tokyo","kansai","hokkaido","kyushu","okinawa","nagoya"]',
      url_template: 'https://www.travel4u.com.tw/group/area/{area_code}/japan/',
      notes: '山富旅遊 - Taiwan OTA with group tours. Area codes map to Japan regions.',
    },
    {
      source_id: 'eztravel',
      name: '易遊網',
      type_json: '["package","flight","hotel"]',
      status: 'active',
      scraper_script: 'scripts/scrape_eztravel.py',
      regions_json: '["tokyo","kansai","nagoya","okinawa","sapporo","fukuoka"]',
      url_template: 'https://packages.eztravel.com.tw/roundtrip-TPE-{dest_code}?checkin={depart_date}&checkout={return_date}&adult={pax}&child=0',
      notes: 'FIT at packages.eztravel.com.tw. Hotels shown may be in different city than destination (e.g., Kobe for Osaka). Check baggage - typically NOT included with LCC flights.',
    },
    {
      source_id: 'jalan',
      name: 'じゃらん',
      type_json: '["hotel"]',
      status: 'inactive',
      scraper_script: null,
      regions_json: null,
      url_template: 'https://www.jalan.net',
      notes: 'Japan domestic OTA - for local hotel bookings',
    },
    {
      source_id: 'rakuten_travel',
      name: '楽天トラベル',
      type_json: '["hotel","package"]',
      status: 'inactive',
      scraper_script: null,
      regions_json: null,
      url_template: 'https://travel.rakuten.co.jp',
      notes: 'Japan domestic OTA',
    },
    {
      source_id: 'trip',
      name: 'Trip.com',
      type_json: '["flight"]',
      status: 'active',
      scraper_script: 'scripts/scrape_package.py',
      regions_json: null,
      url_template: 'https://www.trip.com/flights/{origin}-to-{dest}/tickets-{origin_code}-{dest_code}?dcity={origin_code}&acity={dest_code}&ddate={depart_date}&flighttype=ow&class=y&quantity={pax}',
      notes: 'Flight search works via Playwright. Hotel details require login. Roundtrip search only shows outbound - scrape return as separate one-way. Prices in USD, convert to TWD (~32).',
    },
    {
      source_id: 'booking',
      name: 'Booking.com',
      type_json: '["hotel"]',
      status: 'inactive',
      scraper_script: 'scripts/scrape_package.py',
      regions_json: null,
      url_template: 'https://www.booking.com/searchresults.{lang}.html?dest_id={dest_id}&dest_type=city&checkin={checkin}&checkout={checkout}&group_adults={adults}&no_rooms={rooms}&selected_currency={currency}',
      notes: 'Cloudflare bot protection. Use lang=zh-tw for Traditional Chinese. Initial search may not load results - retry or use direct hotel URL. Prices shown per night.',
    },
    {
      source_id: 'agoda',
      name: 'Agoda',
      type_json: '["hotel"]',
      status: 'active',
      scraper_script: 'scripts/scrape_package.py',
      regions_json: null,
      url_template: 'https://www.agoda.com/{hotel_slug}/hotel/{city_slug}-{country}.html?checkIn={checkin}&los={nights}&adults={adults}&rooms={rooms}&currency={currency}',
      notes: 'Direct hotel page URLs work reliably — full pricing, reviews, amenities. Search pages may return 0 results for dates >6 months out. Use hotel_page URL template for best results.',
    },
    {
      source_id: 'skyscanner',
      name: 'Skyscanner',
      type_json: '["flight"]',
      status: 'inactive',
      scraper_script: null,
      regions_json: null,
      url_template: 'https://www.skyscanner.com.tw',
      notes: 'Strong bot detection - returns captcha page. Use Google Flights instead for multi-airline price comparison.',
    },
    {
      source_id: 'google_flights',
      name: 'Google Flights',
      type_json: '["flight"]',
      status: 'active',
      scraper_script: 'scripts/scrape_package.py',
      regions_json: null,
      url_template: 'https://www.google.com/travel/flights?q=Flights+to+{dest}+from+{origin}+on+{depart_date}+through+{return_date}&curr={currency}&hl=zh-TW',
      notes: 'Uses natural-language query URL format. Returns all-inclusive TWD prices with airline, times, duration, nonstop flags, and CO2 data. No form interaction needed.',
    },
  ];

  for (const src of otaSources) {
    try {
      const esc = (s: string | null | undefined) =>
        s !== null && s !== undefined ? `'${String(s).replace(/'/g, "''")}'` : 'NULL';
      // Scalar columns only; type/regions go to child tables (no JSON in columns).
      await client.execute(
        `INSERT OR IGNORE INTO ota_sources (source_id, name, status, scraper_script, url_template, notes) VALUES (${esc(src.source_id)}, ${esc(src.name)}, ${esc(src.status)}, ${esc(src.scraper_script)}, ${esc(src.url_template)}, ${esc(src.notes)})`
      );
      // Parse the source-code list literals ONCE here (migration code) → child rows.
      const types: string[] = src.type_json ? JSON.parse(src.type_json) : [];
      const regions: string[] = src.regions_json ? JSON.parse(src.regions_json) : [];
      await client.execute(`DELETE FROM ota_source_types WHERE source_id = ${esc(src.source_id)}`);
      for (const t of types) {
        await client.execute(`INSERT OR IGNORE INTO ota_source_types (source_id, type) VALUES (${esc(src.source_id)}, ${esc(t)})`);
      }
      await client.execute(`DELETE FROM ota_source_regions WHERE source_id = ${esc(src.source_id)}`);
      for (const r of regions) {
        await client.execute(`INSERT OR IGNORE INTO ota_source_regions (source_id, region) VALUES (${esc(src.source_id)}, ${esc(r)})`);
      }
      console.log(`✅ Seeded ota_sources: ${src.source_id}`);
    } catch (e: any) {
      console.warn(`⚠️  Could not seed ota_sources ${src.source_id}:`, e.message);
    }
  }

  // ── Shaping Stage — Triangle Research (unscoped: keyed by run_id, not plan_id) ──
  console.log('Creating Shaping Stage research tables...');
  await client.executeMany([
    `CREATE TABLE IF NOT EXISTS shaping_research_runs (
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
    );`,
    `CREATE TABLE IF NOT EXISTS shaping_research_destinations (
      run_id TEXT NOT NULL,
      dest_code TEXT NOT NULL,
      dest_label TEXT NOT NULL,
      sort_order INTEGER NOT NULL,
      PRIMARY KEY (run_id, dest_code)
    );`,
    `CREATE TABLE IF NOT EXISTS shaping_research_durations (
      run_id TEXT NOT NULL,
      nights INTEGER NOT NULL,
      duration_days INTEGER NOT NULL,
      PRIMARY KEY (run_id, nights)
    );`,
    `CREATE TABLE IF NOT EXISTS shaping_candidates (
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
    );`,
    `CREATE TABLE IF NOT EXISTS shaping_candidate_flights (
      candidate_id TEXT NOT NULL,
      direction TEXT NOT NULL,
      airline TEXT,
      depart_time TEXT,
      arrive_time TEXT,
      duration TEXT,
      nonstop INTEGER,
      price_total_twd INTEGER,
      PRIMARY KEY (candidate_id, direction)
    );`,
    `CREATE TABLE IF NOT EXISTS shaping_scrape_attempts (
      run_id TEXT NOT NULL,
      dest_code TEXT NOT NULL,
      nights INTEGER NOT NULL,
      status TEXT NOT NULL,
      candidate_count INTEGER,
      error TEXT,
      attempted_at TEXT,
      PRIMARY KEY (run_id, dest_code, nights)
    );`,
  ]);
  await client.execute('CREATE INDEX IF NOT EXISTS idx_s0_cand_run ON shaping_candidates(run_id, rank);');

  // Shaping Stage Research Shaping (normalized, no JSON).
  // Captures everything that shapes the research search space during the dynamic pre-lock phase.
  // This includes hard constraints (e.g. Liko's 馬偕 date block), soft preferences,
  // search directives, observed signals, and hypotheses.
  // "constraint" is one possible role (others: soft_preference, search_directive, observed_signal...).
  // Fully relational child table — multiple values per (aspect, role, kind) supported by multiple rows.
  await client.execute(`CREATE TABLE IF NOT EXISTS shaping_rules (
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
  );`);
  await client.execute('CREATE INDEX IF NOT EXISTS idx_s0_shaping_run ON shaping_rules(run_id, aspect, role);');

  // Enforce business uniqueness using expression index.
  // COALESCE is allowed in INDEXes (unlike PRIMARY KEY definitions).
  // This prevents truly duplicate shaping rows while still allowing different values
  // for the same (aspect, role, kind) — e.g. multiple preferred_depart dates.
  await client.execute(`CREATE UNIQUE INDEX IF NOT EXISTS uq_s0_shaping_value
    ON shaping_rules(
      run_id, aspect, role, kind,
      COALESCE(value_text, ''),
      COALESCE(value_date, ''),
      COALESCE(value_integer, 0)
    );`);

  console.log('✅ Shaping Stage research tables ready.');

  await client.execute(`CREATE TABLE IF NOT EXISTS shaping_tour_group_offers (
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
    raw_json TEXT,
    parse_warnings_json TEXT,
    PRIMARY KEY (run_id, offer_id)
  );`);

  await client.execute(`CREATE TABLE IF NOT EXISTS shaping_tour_group_scrape_attempts (
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
  );`);

  await client.execute(
    'CREATE INDEX IF NOT EXISTS idx_s0_tg_offers_lookup ON shaping_tour_group_offers(run_id, dest_region, nights, price_per_person_twd);'
  );

  await client.execute(`CREATE TABLE IF NOT EXISTS shaping_research_artifacts (
    artifact_id TEXT PRIMARY KEY,
    run_id TEXT,
    destination_slug TEXT,
    artifact_kind TEXT,
    original_filename TEXT,
    payload_json TEXT,
    raw_text TEXT,
    observed_by TEXT,
    observed_at TEXT,
    imported_at TEXT
  );`);

  await client.execute(`CREATE TABLE IF NOT EXISTS shaping_selected_offers (
    selection_id TEXT PRIMARY KEY,
    run_id TEXT,
    destination_slug TEXT,
    source_id TEXT,
    source_offer_id TEXT,
    selected_depart_date TEXT,
    selected_return_date TEXT,
    nights INTEGER,
    price_per_person_twd INTEGER,
    price_total_twd INTEGER,
    hotel_name TEXT,
    observed_by TEXT,
    observed_at TEXT,
    selected_by TEXT,
    selected_at TEXT,
    provenance_json TEXT,
    raw_json TEXT,
    imported_at TEXT
  );`);

  console.log('✅ Shaping artifact and selected-offer tables ready.');

  // Distinguish 跟團 (group_tour) from 機加酒/自由行 (fit) on the listing page.
  // Both appear under the same listing URLs; only the title reliably tells them
  // apart. Default to 'group_tour' on backfill — that's the safe assumption,
  // since the listing page is positioned as tour-group inventory.
  try {
    await client.execute(`ALTER TABLE shaping_tour_group_offers ADD COLUMN product_kind TEXT NOT NULL DEFAULT 'group_tour';`);
  } catch (e: any) {
    if (!String(e?.message || '').match(/duplicate column name/i)) throw e;
  }
  console.log('✅ Shaping Stage tour-group tables ready.');

  try {
    await client.execute(`ALTER TABLE plan_offers ADD COLUMN package_subtype TEXT;`);
  } catch (e: any) {
    if (!String(e?.message || '').match(/duplicate column name/i)) throw e;
  }
  await client.execute(
    `UPDATE plan_offers SET package_subtype = 'fit' WHERE package_subtype IS NULL;`
  );

  await client.execute(
    `CREATE TABLE IF NOT EXISTS plan_offer_group_meta (
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
    );`
  );
  console.log('✅ Plan-side group-meta table ready.');

  await client.execute(`CREATE TABLE IF NOT EXISTS offers (
    id TEXT,
    source_file TEXT,
    source_id TEXT,
    type TEXT,
    name TEXT,
    price_per_person INTEGER,
    currency TEXT,
    region TEXT,
    destination TEXT,
    departure_date TEXT,
    return_date TEXT,
    nights INTEGER,
    availability TEXT,
    hotel_name TEXT,
    airline TEXT,
    raw_data TEXT,
    scraped_at TEXT,
    PRIMARY KEY (id, scraped_at)
  );`);
  console.log('✅ Offers table ready.');

  await client.execute(`CREATE TABLE IF NOT EXISTS hotel_areas (
    region TEXT,
    area_type TEXT,
    keywords_json TEXT,
    source_url TEXT,
    fetched_at TEXT,
    confidence TEXT,
    PRIMARY KEY (region, area_type)
  );`);

  await client.execute(`CREATE TABLE IF NOT EXISTS transport_routes (
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
  );`);

  await client.execute(`CREATE TABLE IF NOT EXISTS transport_hubs (
    region TEXT,
    hub_id TEXT,
    hub_type TEXT,
    area TEXT,
    source_url TEXT,
    fetched_at TEXT,
    confidence TEXT,
    PRIMARY KEY (region, hub_id)
  );`);

  await client.execute(`CREATE TABLE IF NOT EXISTS destination_references (
    slug TEXT PRIMARY KEY,
    ref_id TEXT,
    payload_json TEXT,
    source_url TEXT,
    fetched_at TEXT,
    confidence TEXT
  );`);
  console.log('✅ Hotel-area and transport-route reference tables ready.');

  // Country-scoped public holiday calendar, fetched live from authoritative gov sources.
  // Replaces the deprecated data/holidays/*.json files (which were hand-curated and
  // unverifiable). Every row records its source so a future query can re-verify or refresh.
  await client.execute(
    `CREATE TABLE IF NOT EXISTS holidays (
      country TEXT,
      year INTEGER,
      date TEXT,
      name TEXT,
      day_of_week TEXT,
      is_holiday INTEGER,
      source_url TEXT,
      source_label TEXT,
      fetched_at TEXT,
      confidence TEXT,
      PRIMARY KEY (country, date)
    );`
  );
  for (const col of [
    `ALTER TABLE holidays ADD COLUMN year INTEGER;`,
    `ALTER TABLE holidays ADD COLUMN confidence TEXT;`,
  ]) {
    try {
      await client.execute(col);
    } catch (e: any) {
      if (!String(e?.message || '').match(/duplicate column name/i)) throw e;
    }
  }
  await client.execute(`UPDATE holidays SET year = CAST(substr(date, 1, 4) AS INTEGER) WHERE year IS NULL AND date IS NOT NULL;`);
  await client.execute(
    'CREATE INDEX IF NOT EXISTS idx_holidays_country_date ON holidays(country, date);'
  );
  console.log('✅ Holidays table ready.');

  // ---------------------------------------------------------------------------
  // OTA domain-knowledge reference tables (replaces ota-knowledge.json).
  // Seeded by scripts/seed-ota-knowledge.ts. No runtime file reads.
  // ---------------------------------------------------------------------------
  await client.execute(`CREATE TABLE IF NOT EXISTS airlines (
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
  );`);

  await client.execute(`CREATE TABLE IF NOT EXISTS booking_types (
    slug TEXT PRIMARY KEY,
    name_zh TEXT,
    description TEXT,
    rules_json TEXT,
    source_url TEXT,
    fetched_at TEXT,
    confidence TEXT
  );`);

  await client.execute(`CREATE TABLE IF NOT EXISTS platform_behaviors (
    platform TEXT PRIMARY KEY,
    currency TEXT,
    price_display TEXT,
    baggage_labels_json TEXT,
    quirks_json TEXT,
    source_url TEXT,
    fetched_at TEXT,
    confidence TEXT
  );`);

  await client.execute(`CREATE TABLE IF NOT EXISTS comparison_rules (
    id INTEGER PRIMARY KEY,
    rule TEXT,
    sort_order INTEGER,
    source_url TEXT,
    fetched_at TEXT,
    confidence TEXT
  );`);
  console.log('✅ OTA knowledge reference tables ready.');

  // ---------------------------------------------------------------------------
  // Normalized destination reference tables (replaces destinations/*.json and
  // the destination_references JSON-blob table). Seeded by
  // scripts/seed-destination-refs.ts. No runtime file reads, no JSON blobs.
  // ---------------------------------------------------------------------------
  // List fields (stations/best_for/tags/pois/tips) live in normalized child
  // tables created in deJsonReferenceData() — no *_json columns here.
  await client.execute(`CREATE TABLE IF NOT EXISTS destination_areas (
    slug TEXT,
    area_id TEXT,
    name TEXT,
    type TEXT,
    vibe TEXT,
    source_url TEXT,
    fetched_at TEXT,
    confidence TEXT,
    PRIMARY KEY (slug, area_id)
  );`);

  await client.execute(`CREATE TABLE IF NOT EXISTS destination_pois (
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
    PRIMARY KEY (slug, poi_id)
  );`);

  await client.execute(`CREATE TABLE IF NOT EXISTS destination_clusters (
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
  );`);

  await client.execute(`CREATE TABLE IF NOT EXISTS destination_transit (
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
  );`);

  // ref_path must NOT point at a local file (no-local-data rule). Reference data
  // now lives in the normalized tables above; clear any legacy file-path values.
  await client.execute(
    `UPDATE destination_config SET ref_path = '' WHERE ref_path LIKE 'src/skills/%';`
  );

  // Supersede the JSON-blob table (no blobs in DB). Data is now normalized.
  await client.execute(`DROP TABLE IF EXISTS destination_references;`);
  console.log('✅ Normalized destination reference tables ready (blob table dropped).');

  await deJsonReferenceData(client);
  await deJsonItinerarySessions(client);

  console.log('Done.');
}

// ---------------------------------------------------------------------------
// Batch C — de-JSON itinerary session/activity list columns.
//
//   timesofday.activities_zh_json → session_activities_zh (one row per item)
//   activities.tags_json          → activity_tags (existing table)
//   timesofday.meals_json         → DROP (already mirrored in session_meals)
//   timesofday.meals_zh_json      → DROP (dead: held non-JSON transit text the
//                                   reader discarded via tryJson→null; the real
//                                   transit text lives in transit_notes_zh)
//
// Idempotent: guarded on activities_zh_json existence; child writes DELETE then
// re-INSERT; column DROP ignores "no such column" on re-run.
// ---------------------------------------------------------------------------
async function deJsonItinerarySessions(client: TursoPipelineClient): Promise<void> {
  console.log('Batch C: de-JSON itinerary session/activity columns...');

  await client.execute(`CREATE TABLE IF NOT EXISTS session_activities_zh (
    plan_id TEXT NOT NULL, destination TEXT NOT NULL,
    day_number INTEGER NOT NULL, session_type TEXT NOT NULL,
    sort_order INTEGER NOT NULL, activity TEXT NOT NULL,
    PRIMARY KEY (plan_id, destination, day_number, session_type, sort_order)
  );`);
  // activity_tags already exists (CLAUDE.md); ensure for fresh DBs.
  await client.execute(`CREATE TABLE IF NOT EXISTS activity_tags (
    activity_id TEXT NOT NULL, tag TEXT NOT NULL,
    updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (activity_id, tag)
  );`);

  const todCols = rowsAt(await client.executeBatch([`PRAGMA table_info(timesofday);`]), 0).map((r) => String(r.name));
  if (!todCols.includes('activities_zh_json')) {
    console.log('  itinerary JSON columns already dropped; skipping backfill.');
  } else {
    const resp = await client.executeBatch([
      `SELECT plan_id, destination, day_number, session_type, activities_zh_json FROM timesofday;`,
      `SELECT id, tags_json FROM activities;`,
    ]);
    const sessions = rowsAt(resp, 0);
    const acts = rowsAt(resp, 1);

    const stmts: Array<{ sql: string; args: ReturnType<typeof tursoText>[] }> = [];

    for (const s of sessions) {
      stmts.push({
        sql: `DELETE FROM session_activities_zh WHERE plan_id = ? AND destination = ? AND day_number = ? AND session_type = ?;`,
        args: [tursoText(s.plan_id), tursoText(s.destination), tursoInt(s.day_number), tursoText(s.session_type)],
      });
      parseJsonArray(s.activities_zh_json).forEach((activity, i) => {
        stmts.push({
          sql: `INSERT INTO session_activities_zh (plan_id, destination, day_number, session_type, sort_order, activity) VALUES (?, ?, ?, ?, ?, ?);`,
          args: [tursoText(s.plan_id), tursoText(s.destination), tursoInt(s.day_number), tursoText(s.session_type), tursoInt(i), tursoText(activity)],
        });
      });
    }

    for (const a of acts) {
      stmts.push({ sql: `DELETE FROM activity_tags WHERE activity_id = ?;`, args: [tursoText(a.id)] });
      parseJsonArray(a.tags_json).forEach((tag) => {
        stmts.push({ sql: `INSERT OR IGNORE INTO activity_tags (activity_id, tag) VALUES (?, ?);`, args: [tursoText(a.id), tursoText(tag)] });
      });
    }

    await client.executeManyParams(stmts);
    console.log(`✅ Batch C backfilled: ${sessions.length} sessions (activities_zh), ${acts.length} activities (tags) → child rows.`);
  }

  const dropCols: Array<[string, string]> = [
    ['timesofday', 'activities_zh_json'],
    ['timesofday', 'meals_json'],
    ['timesofday', 'meals_zh_json'],
    ['activities', 'tags_json'],
  ];
  for (const [table, col] of dropCols) {
    try {
      await client.execute(`ALTER TABLE ${table} DROP COLUMN ${col};`);
      console.log(`  dropped ${table}.${col}`);
    } catch (e: any) {
      if (!/no such column|has no column/i.test(String(e?.message || ''))) {
        console.warn(`⚠️  Could not drop ${table}.${col}:`, e.message);
      }
    }
  }
  console.log('✅ Batch C legacy JSON columns dropped.');
}

// ---------------------------------------------------------------------------
// Batch A — de-JSON the reference-data columns.
//
// Rule (memory: no-json-in-rdb): no JSON-encoded value may live in any RDB
// column. The destination_* / *_config tables stored lists as `*_json` TEXT.
// This migration creates one child row per list element. JSON is parsed here
// ONCE, in one-shot migration code, purely to eliminate it — runtime readers
// will never parse JSON from a column again.
//
// Idempotent: child tables use CREATE TABLE IF NOT EXISTS; each backfill deletes
// the rows it owns for a (slug[,parent_id]) before re-inserting, so re-running
// the migration converges. The legacy `*_json` columns are NOT dropped here —
// they are dropped in a later migration step after all readers/writers cut over
// (drop-after-cutover, per plan).
// ---------------------------------------------------------------------------
async function deJsonReferenceData(client: TursoPipelineClient): Promise<void> {
  console.log('Batch A: de-JSON reference-data columns...');

  // 1. Child tables (one row per list element).
  await client.execute(`CREATE TABLE IF NOT EXISTS destination_area_stations (
    slug TEXT, area_id TEXT, station TEXT, sort_order INTEGER,
    PRIMARY KEY (slug, area_id, sort_order)
  );`);
  await client.execute(`CREATE TABLE IF NOT EXISTS destination_area_best_for (
    slug TEXT, area_id TEXT, tag TEXT, sort_order INTEGER,
    PRIMARY KEY (slug, area_id, sort_order)
  );`);
  await client.execute(`CREATE TABLE IF NOT EXISTS destination_poi_tags (
    slug TEXT, poi_id TEXT, tag TEXT, sort_order INTEGER,
    PRIMARY KEY (slug, poi_id, sort_order)
  );`);
  await client.execute(`CREATE TABLE IF NOT EXISTS destination_cluster_pois (
    slug TEXT, cluster_id TEXT, poi_id TEXT, sort_order INTEGER,
    PRIMARY KEY (slug, cluster_id, sort_order)
  );`);
  await client.execute(`CREATE TABLE IF NOT EXISTS destination_tips (
    slug TEXT, tip TEXT, sort_order INTEGER,
    PRIMARY KEY (slug, sort_order)
  );`);
  await client.execute(`CREATE TABLE IF NOT EXISTS destination_airports (
    slug TEXT, airport TEXT, sort_order INTEGER,
    PRIMARY KEY (slug, sort_order)
  );`);
  await client.execute(`CREATE TABLE IF NOT EXISTS destination_markets (
    slug TEXT, market TEXT, sort_order INTEGER,
    PRIMARY KEY (slug, sort_order)
  );`);
  await client.execute(`CREATE TABLE IF NOT EXISTS origin_airports (
    slug TEXT, airport TEXT, sort_order INTEGER,
    PRIMARY KEY (slug, sort_order)
  );`);

  // 2. Read the legacy JSON columns IF they still exist. On a second run the
  // columns are already dropped; skip the backfill and fall through to the
  // (idempotent) drop step below.
  // Guard on a column read by the FIRST backfill SELECT (destination_areas).
  // If any legacy column is already dropped, the whole batch was applied before.
  const areaCols = rowsAt(await client.executeBatch([`PRAGMA table_info(destination_areas);`]), 0).map((r) => String(r.name));
  if (!areaCols.includes('stations_json')) {
    console.log('  legacy JSON columns already dropped; skipping backfill.');
  } else {
  const resp = await client.executeBatch([
    `SELECT slug, area_id, stations_json, best_for_json FROM destination_areas;`,
    `SELECT slug, poi_id, tags_json FROM destination_pois;`,
    `SELECT slug, cluster_id, pois_json FROM destination_clusters;`,
    `SELECT slug, tips_json, primary_airports_json, markets_json FROM destination_config;`,
    `SELECT slug, primary_airports_json FROM origin_config;`,
  ]);

  const areas = rowsAt(resp, 0);
  const pois = rowsAt(resp, 1);
  const clusters = rowsAt(resp, 2);
  const configs = rowsAt(resp, 3);
  const origins = rowsAt(resp, 4);

  const stmts: Array<{ sql: string; args: ReturnType<typeof tursoText>[] }> = [];

  // destination_areas.stations_json + best_for_json
  for (const r of areas) {
    stmts.push({ sql: `DELETE FROM destination_area_stations WHERE slug = ? AND area_id = ?;`, args: [tursoText(r.slug), tursoText(r.area_id)] });
    stmts.push({ sql: `DELETE FROM destination_area_best_for WHERE slug = ? AND area_id = ?;`, args: [tursoText(r.slug), tursoText(r.area_id)] });
    parseJsonArray(r.stations_json).forEach((station, i) => {
      stmts.push({ sql: `INSERT INTO destination_area_stations (slug, area_id, station, sort_order) VALUES (?, ?, ?, ?);`, args: [tursoText(r.slug), tursoText(r.area_id), tursoText(station), tursoInt(i)] });
    });
    parseJsonArray(r.best_for_json).forEach((tag, i) => {
      stmts.push({ sql: `INSERT INTO destination_area_best_for (slug, area_id, tag, sort_order) VALUES (?, ?, ?, ?);`, args: [tursoText(r.slug), tursoText(r.area_id), tursoText(tag), tursoInt(i)] });
    });
  }

  // destination_pois.tags_json
  for (const r of pois) {
    stmts.push({ sql: `DELETE FROM destination_poi_tags WHERE slug = ? AND poi_id = ?;`, args: [tursoText(r.slug), tursoText(r.poi_id)] });
    parseJsonArray(r.tags_json).forEach((tag, i) => {
      stmts.push({ sql: `INSERT INTO destination_poi_tags (slug, poi_id, tag, sort_order) VALUES (?, ?, ?, ?);`, args: [tursoText(r.slug), tursoText(r.poi_id), tursoText(tag), tursoInt(i)] });
    });
  }

  // destination_clusters.pois_json
  for (const r of clusters) {
    stmts.push({ sql: `DELETE FROM destination_cluster_pois WHERE slug = ? AND cluster_id = ?;`, args: [tursoText(r.slug), tursoText(r.cluster_id)] });
    parseJsonArray(r.pois_json).forEach((poiId, i) => {
      stmts.push({ sql: `INSERT INTO destination_cluster_pois (slug, cluster_id, poi_id, sort_order) VALUES (?, ?, ?, ?);`, args: [tursoText(r.slug), tursoText(r.cluster_id), tursoText(poiId), tursoInt(i)] });
    });
  }

  // destination_config.tips_json + primary_airports_json + markets_json
  for (const r of configs) {
    stmts.push({ sql: `DELETE FROM destination_tips WHERE slug = ?;`, args: [tursoText(r.slug)] });
    stmts.push({ sql: `DELETE FROM destination_airports WHERE slug = ?;`, args: [tursoText(r.slug)] });
    stmts.push({ sql: `DELETE FROM destination_markets WHERE slug = ?;`, args: [tursoText(r.slug)] });
    parseJsonArray(r.tips_json).forEach((tip, i) => {
      stmts.push({ sql: `INSERT INTO destination_tips (slug, tip, sort_order) VALUES (?, ?, ?);`, args: [tursoText(r.slug), tursoText(tip), tursoInt(i)] });
    });
    parseJsonArray(r.primary_airports_json).forEach((airport, i) => {
      stmts.push({ sql: `INSERT INTO destination_airports (slug, airport, sort_order) VALUES (?, ?, ?);`, args: [tursoText(r.slug), tursoText(airport), tursoInt(i)] });
    });
    parseJsonArray(r.markets_json).forEach((market, i) => {
      stmts.push({ sql: `INSERT INTO destination_markets (slug, market, sort_order) VALUES (?, ?, ?);`, args: [tursoText(r.slug), tursoText(market), tursoInt(i)] });
    });
  }

  // origin_config.primary_airports_json
  for (const r of origins) {
    stmts.push({ sql: `DELETE FROM origin_airports WHERE slug = ?;`, args: [tursoText(r.slug)] });
    parseJsonArray(r.primary_airports_json).forEach((airport, i) => {
      stmts.push({ sql: `INSERT INTO origin_airports (slug, airport, sort_order) VALUES (?, ?, ?);`, args: [tursoText(r.slug), tursoText(airport), tursoInt(i)] });
    });
  }

  await client.executeManyParams(stmts);
  console.log(`✅ Batch A backfilled: ${areas.length} areas, ${pois.length} pois, ${clusters.length} clusters, ${configs.length} configs, ${origins.length} origins → child rows.`);
  } // end backfill (legacy columns existed)

  // Drop the legacy JSON columns now that all readers/writers use child tables.
  // libSQL supports ALTER TABLE DROP COLUMN; ignore "no such column" on re-run.
  const dropCols: Array<[string, string]> = [
    ['destination_areas', 'stations_json'],
    ['destination_areas', 'best_for_json'],
    ['destination_pois', 'tags_json'],
    ['destination_clusters', 'pois_json'],
    ['destination_config', 'tips_json'],
    ['destination_config', 'primary_airports_json'],
    ['destination_config', 'markets_json'],
    ['origin_config', 'primary_airports_json'],
    ['ota_sources', 'type_json'],
    ['ota_sources', 'regions_json'],
  ];
  for (const [table, col] of dropCols) {
    try {
      await client.execute(`ALTER TABLE ${table} DROP COLUMN ${col};`);
      console.log(`  dropped ${table}.${col}`);
    } catch (e: any) {
      if (!/no such column|has no column/i.test(String(e?.message || ''))) {
        console.warn(`⚠️  Could not drop ${table}.${col}:`, e.message);
      }
    }
  }
  console.log('✅ Batch A/B legacy JSON columns dropped.');
}

// Read pipeline batch result index into plain row objects.
function rowsAt(resp: any, idx: number): Record<string, any>[] {
  const result = resp?.results?.[idx]?.response?.result;
  if (!result?.rows || !result?.cols) return [];
  const cols = (result.cols as Array<{ name: string }>).map((c) => c.name);
  return (result.rows as unknown[][]).map((row) => {
    const obj: Record<string, any> = {};
    for (let i = 0; i < cols.length; i++) obj[cols[i]] = (row as any)[i]?.value ?? null;
    return obj;
  });
}

// One-shot legacy-JSON decode for the migration only. NOT a runtime path.
function parseJsonArray(v: any): string[] {
  if (v === null || v === undefined || v === '') return [];
  try {
    const parsed = JSON.parse(String(v));
    return Array.isArray(parsed) ? parsed.map((x) => String(x)) : [];
  } catch {
    return [];
  }
}

main().catch(console.error);
