#!/usr/bin/env npx ts-node
/**
 * True Cost Comparison CLI
 *
 * Compares offers by total cost including package price + baggage surcharge + transport cost.
 * Uses Turso for offers, hotel areas, and transport routes. Airline metadata
 * remains in the skill reference ota-knowledge.json.
 *
 * Usage:
 *   npx ts-node src/cli/compare-true-cost.ts --region kansai --date 2026-02-24 --pax 2
 *   npx ts-node src/cli/compare-true-cost.ts --region kansai --date 2026-02-24 --itinerary "kyoto:1,osaka:2"
 */

import * as fs from 'fs';
import * as path from 'path';
import { loadHotelAreasFromTurso, loadTransportRoutesFromTurso, queryOffers } from '../services/turso-service';

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

interface TransportRoute {
  time_min: number;
  cost_jpy: number;
  method: string;
}

interface TransportRoutes {
  [region: string]: {
    routes: Record<string, TransportRoute>;
    hubs: Record<string, { type: string; area: string }>;
  };
}

interface AirlineInfo {
  code: string;
  type: 'LCC' | 'FSC';
  hand_baggage_kg: number;
  checked_bag_included: boolean;
  checked_bag_cost_twd?: number;
  checked_bag_kg?: number;
}

interface TrueCostOffer {
  file: string;
  source_name: string;
  price_per_person: number;
  currency: string;
  airline: string;
  hotel: string;
  hotel_area_type: string;
  baggage_cost: number;
  transport_cost: number;
  transport_time_min: number;
  true_total: number;
  breakdown: string;
}

// ---------------------------------------------------------------------------
// Data loaders
// ---------------------------------------------------------------------------

function loadJson<T>(filePath: string): T | null {
  try {
    return JSON.parse(fs.readFileSync(filePath, 'utf-8'));
  } catch { return null; }
}

function loadTransportRoutes(): TransportRoutes {
  throw new Error('loadTransportRoutes must use Turso via loadTransportRoutesFromTurso()');
}

function loadHotelAreas(): Record<string, Record<string, string[]>> {
  throw new Error('loadHotelAreas must use Turso via loadHotelAreasFromTurso()');
}

function loadAirlines(): Record<string, AirlineInfo> {
  const knowledge = loadJson<any>(
    path.join(process.cwd(), 'src', 'skills', 'travel-shared', 'references', 'ota-knowledge.json')
  );
  return knowledge?.airlines || {};
}

// ---------------------------------------------------------------------------
// Hotel area detection (mirrors Python version)
// ---------------------------------------------------------------------------

function detectHotelArea(hotelName: string, region: string, hotelAreas: Record<string, Record<string, string[]>>): string {
  const areas = hotelAreas[region] || {};
  for (const [areaType, keywords] of Object.entries(areas)) {
    for (const kw of keywords) {
      if (hotelName.includes(kw)) return areaType;
    }
  }
  return 'unknown';
}

// Map hotel area type to nearest transport hub
function areaTypeToHub(areaType: string, region: string, routes: TransportRoutes): string {
  const hubs = routes[region]?.hubs || {};
  // Find first hub matching this area type
  for (const [hubId, info] of Object.entries(hubs)) {
    if (info.type === areaType) return hubId;
  }
  // Default to first central hub
  for (const [hubId, info] of Object.entries(hubs)) {
    if (info.type === 'central') return hubId;
  }
  return '';
}

// ---------------------------------------------------------------------------
// Cost calculators
// ---------------------------------------------------------------------------

function lookupAirline(airlineName: string, airlines: Record<string, AirlineInfo>): AirlineInfo | null {
  const lower = airlineName.toLowerCase();
  // Direct key match
  for (const [key, info] of Object.entries(airlines)) {
    if (lower.includes(key.replace(/_/g, ' ')) || lower.includes(info.code.toLowerCase())) {
      return info;
    }
  }
  // Common name mappings
  const nameMap: Record<string, string> = {
    '樂桃': 'peach', 'peach': 'peach',
    '虎航': 'tigerair', 'tigerair': 'tigerair', '台灣虎航': 'tigerair',
    '捷星': 'jetstar', 'jetstar': 'jetstar',
    '長榮': 'eva', 'eva': 'eva', 'eva air': 'eva',
    '華航': 'china_airlines', '中華航空': 'china_airlines', 'china airlines': 'china_airlines',
    '星宇': 'starlux', 'starlux': 'starlux',
    'thai lion': 'thai_lion',
    'thai vietjet': 'thai_vietjet', '泰越捷': 'thai_vietjet', 'vietjet': 'thai_vietjet',
  };
  for (const [name, key] of Object.entries(nameMap)) {
    if (lower.includes(name)) return airlines[key] || null;
  }
  return null;
}

function calcBaggageCost(
  airline: string,
  packageType: string,
  baggageIncluded: boolean | null,
  pax: number,
  airlines: Record<string, AirlineInfo>,
): number {
  // Explicit baggage status overrides package type
  if (baggageIncluded === true) return 0;
  if (baggageIncluded === false) {
    // EzTravel FIT doesn't include baggage - use airline lookup
    const info = lookupAirline(airline, airlines);
    if (!info || info.checked_bag_included) return 0;
    const costPerSegment = info.checked_bag_cost_twd || 600;
    return 2 * pax * costPerSegment;
  }

  // Null/unknown: FIT packages typically include baggage
  if (packageType === 'fit') return 0;

  const info = lookupAirline(airline, airlines);
  if (!info) return 0;
  if (info.checked_bag_included) return 0;

  // LCC without included baggage: 2 directions × pax × cost
  const costPerSegment = info.checked_bag_cost_twd || 600;
  return 2 * pax * costPerSegment;
}

function lookupRoute(from: string, to: string, regionRoutes: Record<string, TransportRoute>): TransportRoute | null {
  const key1 = `${from}-${to}`;
  const key2 = `${to}-${from}`;
  return regionRoutes[key1] || regionRoutes[key2] || null;
}

function calcTransportCost(
  hotelHub: string,
  itinerary: Map<string, number>,
  region: string,
  pax: number,
  routes: TransportRoutes,
  jpy2twd: number,
): { cost: number; time_min: number } {
  const regionData = routes[region];
  if (!regionData || !hotelHub) return { cost: 0, time_min: 0 };

  let totalJpy = 0;
  let totalMin = 0;

  // Airport transfer: 2 trips (arrival + departure)
  const airportHub = Object.entries(regionData.hubs).find(([, v]) => v.type === 'airport')?.[0];
  if (airportHub) {
    const airportRoute = lookupRoute(airportHub, hotelHub, regionData.routes);
    if (airportRoute) {
      totalJpy += airportRoute.cost_jpy * pax * 2;
      totalMin += airportRoute.time_min * 2;
    }
  }

  // Daily itinerary transport
  for (const [dest, days] of itinerary) {
    const route = lookupRoute(hotelHub, dest, regionData.routes);
    if (route) {
      // Round trip per day
      totalJpy += route.cost_jpy * pax * 2 * days;
      totalMin += route.time_min * 2 * days;
    }
  }

  return { cost: Math.round(totalJpy * jpy2twd), time_min: totalMin };
}

// ---------------------------------------------------------------------------
// Offer scanner (simplified from travel-update.ts)
// ---------------------------------------------------------------------------

interface RawOffer {
  file: string;
  source_id: string;
  price_per_person: number;
  currency: string;
  airline: string;
  hotel: string;
  package_type: string;
  baggage_included: boolean | null;
}

async function scanOffers(region: string, filterDate: string | undefined, pax: number): Promise<RawOffer[]> {
  const rows = await queryOffers({
    region,
    ...(filterDate ? { start: filterDate, end: filterDate } : {}),
    type: 'package',
    limit: 500,
  });

  const offers: RawOffer[] = rows
    .filter((row) => row.price_per_person !== null)
    .map((row) => ({
      file: row.id,
      source_id: row.source_id,
      price_per_person: row.price_per_person!,
      currency: row.currency || 'TWD',
      airline: row.airline || '',
      hotel: row.hotel_name || row.name || '',
      package_type: 'fit',
      baggage_included: null,
    }));
  offers.sort((a, b) => a.price_per_person - b.price_per_person);
  return offers;
}

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

function parseItinerary(spec: string): Map<string, number> {
  const map = new Map<string, number>();
  for (const part of spec.split(',')) {
    const [dest, days] = part.trim().split(':');
    if (dest && days) map.set(dest.trim(), parseInt(days, 10));
  }
  return map;
}

async function main(): Promise<void> {
  const args = process.argv.slice(2);
  const opt = (name: string) => {
    const i = args.indexOf(name);
    return i >= 0 && i + 1 < args.length ? args[i + 1] : undefined;
  };

  const region = opt('--region');
  if (!region) {
    console.error('Usage: compare-true-cost --region <name> [--date YYYY-MM-DD] [--pax N] [--itinerary "kyoto:1,osaka:2"] [--jpy-rate N]');
    process.exit(1);
  }

  const filterDate = opt('--date');
  const pax = parseInt(opt('--pax') || '2', 10);
  const itinerarySpec = opt('--itinerary');
  const jpyRate = parseFloat(opt('--jpy-rate') || '0.22'); // JPY → TWD

  // Load reference data
  const routes = await loadTransportRoutesFromTurso() as TransportRoutes;
  const hotelAreas = await loadHotelAreasFromTurso();
  const airlines = loadAirlines();

  // Scan offers
  const rawOffers = await scanOffers(region, filterDate, pax);
  if (rawOffers.length === 0) {
    console.log(`No offers found for region "${region}".`);
    process.exit(1);
  }

  // Parse itinerary destinations
  const itinerary = itinerarySpec ? parseItinerary(itinerarySpec) : new Map<string, number>();

  // Calculate true cost for each offer
  const results: TrueCostOffer[] = [];

  for (const o of rawOffers) {
    const areaType = o.hotel
      ? detectHotelArea(o.hotel, region, hotelAreas)
      : 'unknown';
    const hotelHub = areaTypeToHub(areaType, region, routes);

    const baggageCost = calcBaggageCost(o.airline, o.package_type, o.baggage_included, pax, airlines);

    const transport = calcTransportCost(hotelHub, itinerary, region, pax, routes, jpyRate);

    const trueTotalPerPerson = o.price_per_person + Math.round((baggageCost + transport.cost) / pax);

    results.push({
      file: o.file,
      source_name: o.source_id,
      price_per_person: o.price_per_person,
      currency: o.currency,
      airline: o.airline,
      hotel: o.hotel,
      hotel_area_type: areaType,
      baggage_cost: baggageCost,
      transport_cost: transport.cost,
      transport_time_min: transport.time_min,
      true_total: trueTotalPerPerson,
      breakdown: `pkg:${o.price_per_person} bag:${baggageCost} xport:${transport.cost}`,
    });
  }

  results.sort((a, b) => a.true_total - b.true_total);

  // Print results
  const bestTime = Math.min(...results.map(r => r.transport_time_min));

  console.log(`\n╔════════════════════════════════════════════════════════════════════════════════════╗`);
  console.log(`║  TRUE COST COMPARISON: ${region.toUpperCase().padEnd(57)}║`);
  console.log(`╚════════════════════════════════════════════════════════════════════════════════════╝`);
  console.log(`  Pax: ${pax} | Date: ${filterDate || '(best)'} | JPY rate: ${jpyRate}`);
  if (itinerarySpec) console.log(`  Itinerary: ${itinerarySpec}`);
  console.log('');

  console.log('┌──────────┬──────────┬─────────┬───────────┬────────────┬──────────┬──────────────────────┐');
  console.log('│ Source   │ Package  │ Baggage │ Transport │ TRUE/person│ Time     │ Hotel                │');
  console.log('├──────────┼──────────┼─────────┼───────────┼────────────┼──────────┼──────────────────────┤');

  for (const r of results) {
    const src = r.source_name.slice(0, 8).padEnd(8);
    const pkg = r.price_per_person.toLocaleString().padStart(8);
    const bag = r.baggage_cost > 0 ? r.baggage_cost.toLocaleString().padStart(7) : '      0';
    const xport = r.transport_cost > 0 ? r.transport_cost.toLocaleString().padStart(9) : '        0';
    const total = r.true_total.toLocaleString().padStart(10);
    const timeDiff = r.transport_time_min - bestTime;
    const timeStr = timeDiff === 0 ? 'optimal ' : `+${timeDiff}min`.padEnd(8);
    const hotel = (r.hotel || '-').slice(0, 20).padEnd(20);

    console.log(`│ ${src} │ ${pkg} │ ${bag} │ ${xport} │ ${total} │ ${timeStr} │ ${hotel} │`);
  }

  console.log('└──────────┴──────────┴─────────┴───────────┴────────────┴──────────┴──────────────────────┘');

  // Recommendation
  if (results.length > 0) {
    const best = results[0];
    console.log(`\n💡 Best true value: ${best.source_name} at ${best.currency} ${best.true_total.toLocaleString()}/person`);
    console.log(`   ${best.breakdown}`);
    if (best.hotel) console.log(`   Hotel: ${best.hotel} (${best.hotel_area_type})`);
    if (best.airline) console.log(`   Airline: ${best.airline}`);

    // Warn if cheapest package != cheapest true cost
    const cheapestPkg = [...results].sort((a, b) => a.price_per_person - b.price_per_person)[0];
    if (cheapestPkg.file !== best.file) {
      console.log(`\n⚠️  Cheapest package (${cheapestPkg.source_name} ${cheapestPkg.price_per_person.toLocaleString()}) is NOT the best true value!`);
      console.log(`   Hidden costs: baggage ${cheapestPkg.baggage_cost.toLocaleString()} + transport ${cheapestPkg.transport_cost.toLocaleString()}`);
    }
  }

  console.log('');
}

main().catch((err) => {
  console.error(err instanceof Error ? err.message : String(err));
  process.exit(1);
});
