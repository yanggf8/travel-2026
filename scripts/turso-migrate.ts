import { TursoPipelineClient } from './turso-pipeline';

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

  // 6. Create plan_snapshots table
  try {
    console.log('Creating plan_snapshots table...');
    await client.execute(`CREATE TABLE IF NOT EXISTS plan_snapshots (
  snapshot_id TEXT PRIMARY KEY,
  trip_id TEXT NOT NULL,
  schema_version TEXT NOT NULL,
  plan_json TEXT NOT NULL,
  state_json TEXT,
  created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);`);
    await client.execute('CREATE INDEX IF NOT EXISTS idx_snapshots_trip ON plan_snapshots(trip_id, created_at);');
    console.log('✅ Created plan_snapshots table.');
  } catch (e: any) {
    if (e.message?.includes('already exists')) {
      console.log('ℹ️  plan_snapshots table already exists.');
    } else {
      console.warn('⚠️  Could not create plan_snapshots table:', e.message);
    }
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

  // 9. Create plans_current table (DB-primary plan storage)
  try {
    console.log('Creating plans_current table...');
    await client.execute(`CREATE TABLE IF NOT EXISTS plans_current (
  plan_id TEXT PRIMARY KEY,
  schema_version TEXT NOT NULL,
  plan_json TEXT NOT NULL,
  state_json TEXT,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
);`);
    console.log('✅ Created plans_current table.');
  } catch (e: any) {
    if (e.message?.includes('already exists')) {
      console.log('ℹ️  plans_current table already exists.');
    } else {
      console.warn('⚠️  Could not create plans_current table:', e.message);
    }
  }

  // 10. Create normalized itinerary tables (Phase 1)
  const itineraryTables: Array<{ name: string; sql: string }> = [
    {
      name: 'itinerary_days',
      sql: `CREATE TABLE IF NOT EXISTS itinerary_days (
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
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination, day_number)
);`,
    },
    {
      name: 'itinerary_sessions',
      sql: `CREATE TABLE IF NOT EXISTS itinerary_sessions (
  plan_id TEXT NOT NULL,
  destination TEXT NOT NULL,
  day_number INTEGER NOT NULL,
  session_type TEXT NOT NULL CHECK(session_type IN ('morning', 'afternoon', 'evening')),
  focus TEXT,
  transit_notes TEXT,
  booking_notes TEXT,
  meals_json TEXT,
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

  console.log('Done.');
}

main().catch(console.error);
