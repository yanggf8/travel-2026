#!/usr/bin/env tsx
import * as fs from 'fs';
const envFile = '/home/yanggf/b/travel-2026/.env';
for (const line of fs.readFileSync(envFile, 'utf8').split('\n')) {
  const m = line.match(/^([^#=]+)=(.*)$/);
  if (m) process.env[m[1].trim()] = m[2].trim();
}
const TURSO_URL = process.env.TURSO_URL!.replace('libsql://', 'https://');
const TURSO_TOKEN = process.env.TURSO_TOKEN!;

const requests = [
  // 1. Rename old tables
  { type: 'execute', stmt: { sql: 'ALTER TABLE itinerary_sessions RENAME TO itinerary_sessions_old' } },
  { type: 'execute', stmt: { sql: 'ALTER TABLE activities RENAME TO activities_old' } },
  // 2. Create new itinerary_sessions with noon in CHECK
  { type: 'execute', stmt: { sql: `CREATE TABLE itinerary_sessions (
    plan_id TEXT NOT NULL,
    destination TEXT NOT NULL,
    day_number INTEGER NOT NULL,
    session_type TEXT NOT NULL CHECK(session_type IN ('morning','noon','afternoon','evening')),
    focus TEXT,
    transit_notes TEXT,
    booking_notes TEXT,
    meals_json TEXT,
    time_range_start TEXT,
    time_range_end TEXT,
    focus_zh TEXT,
    transit_notes_zh TEXT,
    meals_zh_json TEXT,
    activities_zh_json TEXT,
    updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (plan_id, destination, day_number, session_type)
  )` } },
  // 3. Copy sessions data
  { type: 'execute', stmt: { sql: 'INSERT INTO itinerary_sessions SELECT * FROM itinerary_sessions_old' } },
  // 4. Create new activities with noon in CHECK
  { type: 'execute', stmt: { sql: `CREATE TABLE activities (
    id TEXT PRIMARY KEY,
    plan_id TEXT NOT NULL,
    destination TEXT NOT NULL,
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
    tags_json TEXT,
    notes TEXT,
    priority TEXT NOT NULL DEFAULT 'want' CHECK(priority IN ('must','want','optional')),
    updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
  )` } },
  // 5. Copy activities data
  { type: 'execute', stmt: { sql: 'INSERT INTO activities SELECT * FROM activities_old' } },
  // 6. Drop old tables
  { type: 'execute', stmt: { sql: 'DROP TABLE activities_old' } },
  { type: 'execute', stmt: { sql: 'DROP TABLE itinerary_sessions_old' } },
  // 7. Recreate indexes
  { type: 'execute', stmt: { sql: 'CREATE INDEX IF NOT EXISTS idx_activities_session ON activities(plan_id, destination, day_number, session_type, sort_order)' } },
  { type: 'execute', stmt: { sql: 'CREATE INDEX IF NOT EXISTS idx_activities_booking ON activities(plan_id, booking_status)' } },
  { type: 'close' },
];

async function run() {
  const res = await fetch(`${TURSO_URL}/v2/pipeline`, {
    method: 'POST',
    headers: { Authorization: `Bearer ${TURSO_TOKEN}`, 'Content-Type': 'application/json' },
    body: JSON.stringify({ requests }),
  });
  if (!res.ok) throw new Error(`HTTP ${res.status}: ${await res.text()}`);
  const json = await res.json() as any;
  const labels = [
    'rename sessions→old', 'rename activities→old',
    'create itinerary_sessions', 'copy sessions',
    'create activities', 'copy activities',
    'drop activities_old', 'drop sessions_old',
    'idx_activities_session', 'idx_activities_booking',
  ];
  json.results?.slice(0, -1).forEach((r: any, i: number) => {
    const error = r?.response?.error;
    if (error) console.error(`❌ [${labels[i]}]: ${JSON.stringify(error)}`);
    else console.log(`✅ [${labels[i]}]`);
  });
}
run().catch(e => { console.error(e); process.exit(1); });
