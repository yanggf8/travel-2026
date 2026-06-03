/**
 * Flight Data Normalizer
 *
 * Parses scraped Trip.com flight search JSON into structured flight options.
 * Handles both outbound (TPE→KIX) and return (KIX→TPE) data.
 *
 * Usage:
 *   npx ts-node src/utils/flight-normalizer.ts --scan 2026-02-24 2026-02-28
 */

import { Result } from '../types';
import { EXCHANGE_RATES, convertToTWD } from '../config/constants';
import { queryRawOffers, TursoRawOfferResult } from '../services/turso-service';

// Types
export interface NormalizedFlight {
  airline: string;
  depTime: string;
  arrTime: string;
  depAirport: string;
  arrAirport: string;
  duration: string;
  nonstop: boolean;
  pricePerPerson: number;
  priceTotal: number;
  currency: string;
  priceTotalTWD: number;
  baggageIncluded: boolean;
  isLCC: boolean;
}

export interface FlightSearchResult {
  file: string;
  direction: 'outbound' | 'return';
  date: string;
  pax: number;
  flights: NormalizedFlight[];
  cheapestLCC: NormalizedFlight | null;
  cheapestFull: NormalizedFlight | null;
  cheapestAny: NormalizedFlight | null;
}

const LCC_AIRLINES = new Set([
  'Peach',
  'Tigerair Taiwan',
  'Jetstar Japan',
  'AirAsia X Berhad',
  'Thai Vietjet Air',
  'Thai Lion Air',
  'HK Express',
  'Scoot',
]);

const FULL_SERVICE_AIRLINES = new Set([
  'EVA Air',
  'China Airlines',
  'Cathay Pacific',
  'STARLUX Airlines',
  'Japan Airlines',
  'ANA',
  'All Nippon Airways',
  'Hong Kong Airlines',
]);

function isLCCAirline(airline: string): boolean {
  return LCC_AIRLINES.has(airline);
}

/**
 * Parse Trip.com raw_text to extract nonstop flight options.
 */
function parseFlights(rawText: string, pax: number): NormalizedFlight[] {
  const flights: NormalizedFlight[] = [];

  // Match pattern: airline info followed by price
  // Format in raw text:
  //   [Carry-on baggage included | Included]
  //   Airline Name
  //   HH:MM
  //   APT TX
  //   Xh Ym
  //   Nonstop
  //   HH:MM
  //   APT TX
  //   US$XXX
  //   Total US$XXX

  // Split by "Select" which separates each flight option
  const sections = rawText.split('\nSelect\n');

  for (const section of sections) {
    // Check if this section has a nonstop flight
    if (!section.includes('Nonstop')) continue;

    // Determine baggage status from the section
    // "Included" without "Carry-on" prefix means checked baggage included
    const lines = section.split('\n');
    let baggageIncluded = false;

    for (const line of lines) {
      const trimmed = line.trim();
      if (trimmed === 'Included') {
        baggageIncluded = true;
        break;
      }
      if (trimmed === 'Carry-on baggage included') {
        baggageIncluded = false;
        break;
      }
    }

    // Extract flight details using regex on the section
    const flightMatch = section.match(
      /(?:^|\n)([A-Za-z][\w\s]+?)\n(\d{2}:\d{2})\n([A-Z]{3} T\d)\n(\d+h \d+m)\nNonstop\n(\d{2}:\d{2})\n([A-Z]{3} T\d)/
    );

    if (!flightMatch) continue;

    const [, airlineRaw, depTime, depAirport, duration, arrTime, arrAirport] = flightMatch;
    const airline = airlineRaw.trim().split('\n').pop()!.trim();

    // Extract price
    const priceMatch = section.match(/US\$(\d[\d,]*)\nTotal US\$(\d[\d,]*)/);
    if (!priceMatch) continue;

    const pricePerPerson = parseInt(priceMatch[1].replace(/,/g, ''), 10);
    const priceTotal = parseInt(priceMatch[2].replace(/,/g, ''), 10);
    const priceTotalTWD = convertToTWD(priceTotal, 'USD');

    const isLCC = isLCCAirline(airline);

    flights.push({
      airline,
      depTime,
      arrTime,
      depAirport,
      arrAirport,
      duration,
      nonstop: true,
      pricePerPerson,
      priceTotal,
      currency: 'USD',
      priceTotalTWD,
      baggageIncluded,
      isLCC,
    });
  }

  // Sort by total price TWD
  flights.sort((a, b) => a.priceTotalTWD - b.priceTotalTWD);

  return flights;
}

/**
 * Detect flight direction and date from URL or content.
 */
function detectDirectionAndDate(data: Record<string, unknown>): { direction: 'outbound' | 'return'; date: string } {
  const url = (data.url as string) || '';

  // Check URL for direction
  const isOutbound = url.includes('tpe-kix') || url.includes('dcity=tpe');
  const direction: 'outbound' | 'return' = isOutbound ? 'outbound' : 'return';

  // Extract date from URL
  const dateMatch = url.match(/ddate=(\d{4}-\d{2}-\d{2})/);
  const date = dateMatch ? dateMatch[1] : 'unknown';

  return { direction, date };
}

/**
 * Normalize a Trip.com flight search payload already imported into Turso.
 */
export function normalizeFlightPayload(label: string, data: Record<string, unknown>): Result<FlightSearchResult> {
  try {
    const rawText = (data.raw_text as string) || '';
    if (!rawText) {
      return Result.err(`No raw_text found in ${label}`);
    }

    // Detect pax from URL
    const url = (data.url as string) || '';
    const paxMatch = url.match(/quantity=(\d+)/);
    const pax = paxMatch ? parseInt(paxMatch[1], 10) : 2;

    const { direction, date } = detectDirectionAndDate(data);
    const flights = parseFlights(rawText, pax);

    const lccFlights = flights.filter(f => f.isLCC);
    const fullFlights = flights.filter(f => !f.isLCC);

    return Result.ok({
      file: label,
      direction,
      date,
      pax,
      flights,
      cheapestLCC: lccFlights.length > 0 ? lccFlights[0] : null,
      cheapestFull: fullFlights.length > 0 ? fullFlights[0] : null,
      cheapestAny: flights.length > 0 ? flights[0] : null,
    });
  } catch (e) {
    return Result.err(`Failed to normalize ${label}: ${e instanceof Error ? e.message : String(e)}`);
  }
}

/**
 * Local flight JSON files are not a source of truth. Use Turso-imported offers.
 */
export function normalizeFlightData(filePath: string): Result<FlightSearchResult> {
  return Result.err(`Local flight data files are not supported: ${filePath}. Import scrape output with npm run db:import:turso and use --scan.`);
}

export function scanFlightFiles(): { outbound: Map<string, string>; return_: Map<string, string> } {
  throw new Error('Local scrapes/ scans are not supported. Import scrape output with npm run db:import:turso and query Turso offers.');
}

async function loadFlightResultsFromTurso(startDate?: string, endDate?: string): Promise<FlightSearchResult[]> {
  const rows = await queryRawOffers({
    type: 'flight',
    ...(startDate ? { start: startDate } : {}),
    ...(endDate ? { end: endDate } : {}),
    limit: 500,
  });
  const results: FlightSearchResult[] = [];
  for (const row of rows) {
    if (!row.raw_data) continue;
    try {
      const payload = JSON.parse(row.raw_data) as Record<string, unknown>;
      const normalized = normalizeFlightPayload(row.id, payload);
      if (normalized.ok) results.push(normalized.value);
    } catch {
      // Skip malformed imported payloads.
    }
  }
  return results;
}

/**
 * Format a single flight for display.
 */
export function formatFlight(f: NormalizedFlight): string {
  const bag = f.baggageIncluded ? 'bag' : 'carry';
  const type = f.isLCC ? 'LCC' : 'FSC';
  return `${f.airline} ${f.depTime}→${f.arrTime} US$${f.priceTotal}(2p) TWD${f.priceTotalTWD} [${type}/${bag}]`;
}

// CLI entry point
if (require.main === module) {
  (async () => {
    const args = process.argv.slice(2);

    if (args.length === 0 || args.includes('--help') || args.includes('-h')) {
      console.log(`
Flight Data Normalizer

Usage:
  npx ts-node src/utils/flight-normalizer.ts --scan [startDate] [endDate]

Options:
  --top N        Show only top N cheapest flights (default: all)
  --scan         Query Turso flight offers and summarize
`);
      process.exit(0);
    }

    if (args[0] === '--scan') {
      const [, startDate, endDate] = args;
      const results = await loadFlightResultsFromTurso(startDate, endDate);

      console.log('=== Outbound Flights (TPE→KIX) ===');
      for (const r of results.filter((x) => x.direction === 'outbound').sort((a, b) => a.date.localeCompare(b.date))) {
        const cheapest = r.cheapestAny;
        console.log(`  ${r.date}: ${r.flights.length} flights, cheapest: ${cheapest ? formatFlight(cheapest) : 'none'}`);
      }

      console.log('\n=== Return Flights (KIX→TPE) ===');
      for (const r of results.filter((x) => x.direction === 'return').sort((a, b) => a.date.localeCompare(b.date))) {
        const cheapest = r.cheapestAny;
        console.log(`  ${r.date}: ${r.flights.length} flights, cheapest: ${cheapest ? formatFlight(cheapest) : 'none'}`);
      }
    } else {
      console.error('Error: local flight JSON input is not supported. Use --scan after importing offers into Turso.');
      process.exit(1);
    }
  })().catch((err) => {
    console.error(err instanceof Error ? err.message : String(err));
    process.exit(1);
  });
}

export { LCC_AIRLINES, FULL_SERVICE_AIRLINES };
