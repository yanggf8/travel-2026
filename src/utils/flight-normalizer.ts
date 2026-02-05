/**
 * Flight Data Normalizer
 *
 * Parses scraped Trip.com flight search JSON into structured flight options.
 * Handles both outbound (TPE→KIX) and return (KIX→TPE) data.
 *
 * Usage:
 *   npx ts-node src/utils/flight-normalizer.ts data/trip-feb24-out.json
 *   npx ts-node src/utils/flight-normalizer.ts data/trip-feb28-return.json --top 5
 *   npx ts-node src/utils/flight-normalizer.ts --scan 2026-02-24 2026-02-28
 */

import * as fs from 'fs';
import * as path from 'path';
import { Result } from '../types';
import { EXCHANGE_RATES, convertToTWD } from '../config/constants';

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
 * Normalize a Trip.com flight search JSON file.
 */
export function normalizeFlightData(filePath: string): Result<FlightSearchResult> {
  try {
    const absolutePath = path.isAbsolute(filePath)
      ? filePath
      : path.resolve(process.cwd(), filePath);
    const content = fs.readFileSync(absolutePath, 'utf-8');
    const data = JSON.parse(content) as Record<string, unknown>;

    const rawText = (data.raw_text as string) || '';
    if (!rawText) {
      return Result.err(`No raw_text found in ${filePath}`);
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
      file: filePath,
      direction,
      date,
      pax,
      flights,
      cheapestLCC: lccFlights.length > 0 ? lccFlights[0] : null,
      cheapestFull: fullFlights.length > 0 ? fullFlights[0] : null,
      cheapestAny: flights.length > 0 ? flights[0] : null,
    });
  } catch (e) {
    return Result.err(`Failed to normalize ${filePath}: ${e instanceof Error ? e.message : String(e)}`);
  }
}

/**
 * Scan data/ directory for flight data files matching a date range.
 */
export function scanFlightFiles(
  dataDir: string,
  startDate?: string,
  endDate?: string
): { outbound: Map<string, string>; return_: Map<string, string> } {
  const outbound = new Map<string, string>();
  const return_ = new Map<string, string>();

  const dir = path.isAbsolute(dataDir) ? dataDir : path.resolve(process.cwd(), dataDir);
  const files = fs.readdirSync(dir).filter(f => f.startsWith('trip-') && f.endsWith('.json'));

  for (const file of files) {
    const filePath = path.join(dir, file);
    try {
      const content = fs.readFileSync(filePath, 'utf-8');
      const data = JSON.parse(content);
      const url = data.url || '';

      const dateMatch = url.match(/ddate=(\d{4}-\d{2}-\d{2})/);
      if (!dateMatch) continue;

      const date = dateMatch[1];
      const isOutbound = url.includes('dcity=tpe');

      if (isOutbound) {
        // Prefer newer files (v2 suffix)
        if (!outbound.has(date) || file.includes('v2') || file.includes('fresh')) {
          outbound.set(date, filePath);
        }
      } else {
        if (!return_.has(date) || file.includes('v2') || file.includes('fresh')) {
          return_.set(date, filePath);
        }
      }
    } catch {
      // Skip unparseable files
    }
  }

  return { outbound, return_ };
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
  const args = process.argv.slice(2);

  if (args.length === 0 || args.includes('--help') || args.includes('-h')) {
    console.log(`
Flight Data Normalizer

Usage:
  npx ts-node src/utils/flight-normalizer.ts <file.json> [--top N]
  npx ts-node src/utils/flight-normalizer.ts --scan [startDate] [endDate]

Options:
  <file.json>    Parse a single Trip.com flight JSON file
  --top N        Show only top N cheapest flights (default: all)
  --scan         Scan data/ for all flight files and summarize
`);
    process.exit(0);
  }

  if (args[0] === '--scan') {
    const [, startDate, endDate] = args;
    const { outbound, return_ } = scanFlightFiles('data', startDate, endDate);

    console.log('=== Outbound Flights (TPE→KIX) ===');
    for (const [date, file] of [...outbound.entries()].sort()) {
      const result = normalizeFlightData(file);
      if (result.ok) {
        const r = result.value;
        const cheapest = r.cheapestAny;
        console.log(`  ${date}: ${r.flights.length} flights, cheapest: ${cheapest ? formatFlight(cheapest) : 'none'}`);
      }
    }

    console.log('\n=== Return Flights (KIX→TPE) ===');
    for (const [date, file] of [...return_.entries()].sort()) {
      const result = normalizeFlightData(file);
      if (result.ok) {
        const r = result.value;
        const cheapest = r.cheapestAny;
        console.log(`  ${date}: ${r.flights.length} flights, cheapest: ${cheapest ? formatFlight(cheapest) : 'none'}`);
      }
    }
  } else {
    // Single file mode
    const filePath = args[0];
    const topIdx = args.indexOf('--top');
    const topN = topIdx >= 0 ? parseInt(args[topIdx + 1], 10) : Infinity;

    const result = normalizeFlightData(filePath);
    if (!result.ok) {
      console.error(result.error);
      process.exit(1);
    }

    const r = result.value;
    console.log(`File: ${r.file}`);
    console.log(`Direction: ${r.direction}`);
    console.log(`Date: ${r.date}`);
    console.log(`Pax: ${r.pax}`);
    console.log(`Total flights: ${r.flights.length}`);
    console.log('');

    const shown = r.flights.slice(0, topN);
    console.log('| # | Airline | Dep | Arr | USD(2p) | TWD(2p) | Type | Bag |');
    console.log('|---|---------|-----|-----|--------:|--------:|------|-----|');
    shown.forEach((f, i) => {
      const type = f.isLCC ? 'LCC' : 'FSC';
      const bag = f.baggageIncluded ? 'inc' : 'no';
      console.log(`| ${i + 1} | ${f.airline} | ${f.depTime} | ${f.arrTime} | ${f.priceTotal} | ${f.priceTotalTWD} | ${type} | ${bag} |`);
    });
  }
}

export { LCC_AIRLINES, FULL_SERVICE_AIRLINES };
