#!/usr/bin/env npx ts-node
/**
 * True Cost Comparison CLI
 *
 * Compares offers by total cost including package price + baggage surcharge + transport cost.
 * Uses data from: ota-knowledge.json (airlines), transport-routes.json, hotel-areas.json
 *
 * Usage:
 *   npx ts-node src/cli/compare-true-cost.ts --region kansai --date 2026-02-24 --pax 2
 *   npx ts-node src/cli/compare-true-cost.ts --region kansai --date 2026-02-24 --itinerary "kyoto:1,osaka:2"
 */

import * as fs from 'fs';
import * as path from 'path';

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
  return loadJson(path.join(process.cwd(), 'data', 'transport-routes.json')) || {};
}

function loadHotelAreas(): Record<string, Record<string, string[]>> {
  return loadJson(path.join(process.cwd(), 'data', 'hotel-areas.json')) || {};
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

function scanOffers(region: string, filterDate: string | undefined, pax: number): RawOffer[] {
  const dataDir = path.join(process.cwd(), 'data');
  if (!fs.existsSync(dataDir)) return [];

  // Region aliases - files may use city names instead of region
  const regionAliases: Record<string, string[]> = {
    kansai: ['kansai', 'osaka', 'kyoto', 'kobe', 'nara', 'kix'],
    tokyo: ['tokyo', 'tyo', 'nrt', 'hnd'],
    hokkaido: ['hokkaido', 'sapporo', 'cts'],
    okinawa: ['okinawa', 'oka', 'naha'],
  };
  const aliases = regionAliases[region.toLowerCase()] || [region.toLowerCase()];

  const files = fs.readdirSync(dataDir).filter(f => {
    const lower = f.toLowerCase();
    return f.endsWith('.json') &&
      aliases.some(alias => lower.includes(alias)) &&
      !['schema', 'travel-plan', 'destinations', 'ota-sources', 'transport-routes', 'hotel-areas', 'state', 'holidays'].some(x => lower.includes(x));
  });

  const offers: RawOffer[] = [];
  const sourceNames: Record<string, string> = {
    besttour: '喜鴻', liontravel: '雄獅', lifetour: '五福',
    settour: '東南', eztravel: '易遊網', tigerair: '虎航',
  };

  for (const file of files) {
    try {
      const data = JSON.parse(fs.readFileSync(path.join(dataDir, file), 'utf-8'));
      const extracted = data.extracted || {};

      // Price extraction - handle multiple formats
      let pp: number | null = null;
      const dp = extracted.date_pricing || data.date_pricing;
      if (filterDate && dp?.[filterDate]?.price) {
        pp = dp[filterDate].price;
      } else if (dp && typeof dp === 'object') {
        const prices = Object.values(dp).map((v: any) => v.price).filter((p: any) => typeof p === 'number' && p > 0);
        if (prices.length) pp = Math.min(...prices as number[]);
      }
      if (pp == null) pp = extracted.price?.per_person || data.price?.per_person;

      // Priority: offers[] array (Lifetour FIT format) - has individual hotel names
      if (Array.isArray(data.offers) && data.offers.length > 0) {
        for (const offer of data.offers) {
          const offerPrice = offer.price_per_person;
          if (typeof offerPrice === 'number' && offerPrice > 0) {
            offers.push({
              file,
              source_id: offer.source_id || data.source_id || '',
              price_per_person: offerPrice,
              currency: data.currency || 'TWD',
              airline: offer.airline || data.airline || '',
              hotel: offer.hotel || offer.title?.match(/】(.+?)(自由行|飯店)/)?.[1] || '',
              package_type: offer.package_type || data.package_type || 'fit',
              baggage_included: data.baggage_included ?? null,
            });
          }
        }
        continue; // Skip the standard processing since we handled offers
      }

      // Fallback: price_range.min (EzTravel format - no individual offers)
      if (pp == null && data.price_range?.min) pp = data.price_range.min;
      if (pp == null || pp <= 0 || typeof pp !== 'number' || isNaN(pp)) continue;

      // Source ID
      const url = data.url || '';
      let sid = data.source_id || '';
      if (!sid) {
        for (const key of Object.keys(sourceNames)) {
          if (url.toLowerCase().includes(key) || file.toLowerCase().includes(key)) { sid = key; break; }
        }
      }
      if (!sid) continue;

      // Airline - check multiple locations
      let airline = extracted.flight?.outbound?.airline || data.flight?.outbound?.airline || data.airline || '';

      // Hotel
      let hotel = extracted.hotel?.name || data.hotel?.name || '';
      if (!hotel && extracted.hotel?.names?.length) hotel = extracted.hotel.names[0];

      // Package type
      let pkgType = data.package_type || 'unknown';
      if (pkgType === 'unknown' && (url.includes('vacation.liontravel') || url.includes('自由配'))) pkgType = 'fit';

      offers.push({
        file,
        source_id: sid,
        price_per_person: pp,
        currency: data.currency || extracted.price?.currency || 'TWD',
        airline,
        hotel,
        package_type: pkgType,
        baggage_included: data.baggage_included ?? null,
      });
    } catch { /* skip */ }
  }

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

function main(): void {
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
  const routes = loadTransportRoutes();
  const hotelAreas = loadHotelAreas();
  const airlines = loadAirlines();

  // Scan offers
  const rawOffers = scanOffers(region, filterDate, pax);
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

main();
