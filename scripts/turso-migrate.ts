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

  console.log('Done.');
}

main().catch(console.error);
