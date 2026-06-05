/**
 * Seed OTA domain knowledge into Turso.
 *
 * Replaces src/skills/travel-shared/references/ota-knowledge.json. The data is
 * inlined below as typed constants (curated domain knowledge, not fetched), so
 * no JSON file is read at runtime and the file can be deleted. Idempotent via
 * INSERT OR REPLACE.
 *
 * Tables: airlines, booking_types, platform_behaviors, comparison_rules.
 *
 * Usage:
 *   npx ts-node scripts/seed-ota-knowledge.ts
 */

import { TursoPipelineClient, tursoText, tursoInt } from './turso-pipeline';

const SOURCE_URL = 'repo:src/skills/travel-shared/references/ota-knowledge.json#v1.0.0';
const CONFIDENCE = 'curated';
const FETCHED_AT = new Date().toISOString();

// --- airlines -------------------------------------------------------------
// classification_baggage_note / classification_meal fold in the old
// airline_classification (FSC vs LCC) baggage + meal generalizations.
const FSC_BAG = 'Included (typically 23-30kg)';
const FSC_MEAL = 'Included';
const LCC_BAG = 'NOT included (purchase 20kg: ~TWD 1,500-2,500/direction/person). When booked through travel agency FIT, 20kg baggage IS included.';
const LCC_MEAL = 'NOT included';

interface Airline {
  code: string;
  name: string;
  type: 'FSC' | 'LCC';
  hand_baggage_kg: number;
  checked_bag_included: boolean;
  checked_bag_kg: number;
  checked_bag_cost_twd: number | null;
  checked_bag_cost_jpy: number | null;
}

const AIRLINES: Airline[] = [
  { code: 'MM', name: 'Peach', type: 'LCC', hand_baggage_kg: 7, checked_bag_included: false, checked_bag_kg: 20, checked_bag_cost_twd: 550, checked_bag_cost_jpy: 2500 },
  { code: 'SL', name: 'Thai Lion Air', type: 'LCC', hand_baggage_kg: 7, checked_bag_included: false, checked_bag_kg: 20, checked_bag_cost_twd: 650, checked_bag_cost_jpy: 3000 },
  { code: 'IT', name: 'Tigerair Taiwan', type: 'LCC', hand_baggage_kg: 10, checked_bag_included: false, checked_bag_kg: 20, checked_bag_cost_twd: 600, checked_bag_cost_jpy: 2700 },
  { code: 'GK', name: 'Jetstar Japan', type: 'LCC', hand_baggage_kg: 7, checked_bag_included: false, checked_bag_kg: 20, checked_bag_cost_twd: 580, checked_bag_cost_jpy: 2600 },
  { code: 'VZ', name: 'Thai Vietjet Air', type: 'LCC', hand_baggage_kg: 7, checked_bag_included: false, checked_bag_kg: 20, checked_bag_cost_twd: 700, checked_bag_cost_jpy: 3200 },
  { code: 'BR', name: 'EVA Air', type: 'FSC', hand_baggage_kg: 7, checked_bag_included: true, checked_bag_kg: 23, checked_bag_cost_twd: null, checked_bag_cost_jpy: null },
  { code: 'CI', name: 'China Airlines', type: 'FSC', hand_baggage_kg: 7, checked_bag_included: true, checked_bag_kg: 23, checked_bag_cost_twd: null, checked_bag_cost_jpy: null },
  { code: 'JX', name: 'STARLUX Airlines', type: 'FSC', hand_baggage_kg: 7, checked_bag_included: true, checked_bag_kg: 23, checked_bag_cost_twd: null, checked_bag_cost_jpy: null },
];

// --- booking_types --------------------------------------------------------
interface BookingType {
  slug: string;
  name_zh: string;
  description: string;
  rules: string[];
}

const BOOKING_TYPES: BookingType[] = [
  {
    slug: 'fit_package',
    name_zh: 'FIT 自由配',
    description: 'Travel agency bundled flight + hotel, traveler plans own itinerary',
    rules: [
      'Checked baggage (typically 20kg) is ALWAYS included, even for LCC flights',
      'Price is per person unless explicitly stated otherwise',
      'Travel agency provides after-sales support (rebooking, cancellation)',
      'Package may include airport transfers or other extras',
    ],
  },
  {
    slug: 'group_tour',
    name_zh: '團體旅遊',
    description: 'Fully guided group tour with fixed itinerary',
    rules: [
      'All-inclusive: flight, hotel, meals, transport, guide',
      'Fixed departure dates with minimum group size',
      'Price is per person',
    ],
  },
  {
    slug: 'separate_booking',
    name_zh: '分開訂',
    description: 'Book flight and hotel independently',
    rules: [
      'LCC flights typically do NOT include checked baggage',
      'Full-service airlines (EVA, China Airlines, Cathay, JAL, ANA) include checked baggage',
      'Hotel prices may be per room per night, not per person',
      'No single point of contact for support',
    ],
  },
];

// --- platform_behaviors ---------------------------------------------------
interface PlatformBehavior {
  platform: string;
  currency: string;
  price_display: string;
  baggage_labels: Record<string, string>;
  quirks: string[];
}

const PLATFORM_BEHAVIORS: PlatformBehavior[] = [
  {
    platform: 'trip_com',
    currency: 'USD',
    price_display: 'per person for flights, total shown separately',
    baggage_labels: {
      Included: 'Checked baggage included in fare',
      'Carry-on baggage included': 'Only carry-on, NO checked baggage - must purchase separately',
    },
    quirks: [
      'Hotel detail pages require login',
      'Roundtrip search only shows outbound flights',
      'Scrape return as separate one-way search',
      'Prices in USD - convert using EXCHANGE_RATES.USD_TWD',
    ],
  },
  {
    platform: 'booking_com',
    currency: 'TWD',
    price_display: 'per room per night',
    baggage_labels: {},
    quirks: [
      'Cloudflare bot protection - may need retry',
      "Prices shown are 'starting from' and may not reflect actual dates",
      'Taxes and service fees may be separate',
      'Use lang=zh-tw for Traditional Chinese',
    ],
  },
  {
    platform: 'liontravel',
    currency: 'TWD',
    price_display: 'per person for FIT, total shown in search results',
    baggage_labels: {},
    quirks: [
      'FIT search via vacation.liontravel.com',
      'Group tour URL format unknown',
      'Thursday FITPKG promo: TWD 400 off (min TWD 20,000)',
      'Date-specific pricing varies dramatically',
    ],
  },
];

// --- comparison_rules -----------------------------------------------------
const COMPARISON_RULES: string[] = [
  'Always compare same date range across FIT and separate booking',
  'For LCC separate booking: add baggage fee (2 pax × 2 directions × ~TWD 1,750)',
  'For FIT with LCC: baggage is included, do NOT add extra fee',
  'Use leave-calculator with holiday calendar for fair leave day comparison',
  'Convert all prices to TWD before comparing',
  'Hotel comparison: match similar class (e.g. Just Sleep ≈ mid-range business hotel)',
];

async function main(): Promise<void> {
  const client = new TursoPipelineClient();
  const stmts: Array<{ sql: string; args: ReturnType<typeof tursoText>[] }> = [];

  for (const a of AIRLINES) {
    const fsc = a.type === 'FSC';
    stmts.push({
      sql: `INSERT OR REPLACE INTO airlines
        (code, name, type, hand_baggage_kg, checked_bag_included, checked_bag_kg,
         checked_bag_cost_twd, checked_bag_cost_jpy, classification_baggage_note,
         classification_meal, source_url, fetched_at, confidence)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?);`,
      args: [
        tursoText(a.code), tursoText(a.name), tursoText(a.type),
        tursoInt(a.hand_baggage_kg), tursoInt(a.checked_bag_included ? 1 : 0), tursoInt(a.checked_bag_kg),
        tursoInt(a.checked_bag_cost_twd), tursoInt(a.checked_bag_cost_jpy),
        tursoText(fsc ? FSC_BAG : LCC_BAG), tursoText(fsc ? FSC_MEAL : LCC_MEAL),
        tursoText(SOURCE_URL), tursoText(FETCHED_AT), tursoText(CONFIDENCE),
      ],
    });
  }

  for (const b of BOOKING_TYPES) {
    stmts.push({
      sql: `INSERT OR REPLACE INTO booking_types
        (slug, name_zh, description, rules_json, source_url, fetched_at, confidence)
        VALUES (?, ?, ?, ?, ?, ?, ?);`,
      args: [
        tursoText(b.slug), tursoText(b.name_zh), tursoText(b.description),
        tursoText(JSON.stringify(b.rules)),
        tursoText(SOURCE_URL), tursoText(FETCHED_AT), tursoText(CONFIDENCE),
      ],
    });
  }

  for (const p of PLATFORM_BEHAVIORS) {
    stmts.push({
      sql: `INSERT OR REPLACE INTO platform_behaviors
        (platform, currency, price_display, baggage_labels_json, quirks_json,
         source_url, fetched_at, confidence)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?);`,
      args: [
        tursoText(p.platform), tursoText(p.currency), tursoText(p.price_display),
        tursoText(JSON.stringify(p.baggage_labels)), tursoText(JSON.stringify(p.quirks)),
        tursoText(SOURCE_URL), tursoText(FETCHED_AT), tursoText(CONFIDENCE),
      ],
    });
  }

  COMPARISON_RULES.forEach((rule, i) => {
    stmts.push({
      sql: `INSERT OR REPLACE INTO comparison_rules
        (id, rule, sort_order, source_url, fetched_at, confidence)
        VALUES (?, ?, ?, ?, ?, ?);`,
      args: [
        tursoInt(i + 1), tursoText(rule), tursoInt(i + 1),
        tursoText(SOURCE_URL), tursoText(FETCHED_AT), tursoText(CONFIDENCE),
      ],
    });
  });

  console.log(`Writing ${stmts.length} OTA-knowledge rows to Turso...`);
  await client.executeManyParams(stmts);
  console.log(`✅ Seeded: ${AIRLINES.length} airlines, ${BOOKING_TYPES.length} booking_types, ` +
    `${PLATFORM_BEHAVIORS.length} platform_behaviors, ${COMPARISON_RULES.length} comparison_rules.`);
}

main().catch((err) => {
  console.error('Error:', err);
  process.exit(1);
});
