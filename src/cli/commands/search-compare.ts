import type { CommandHandler, CliContext } from '../shared/types';
import { registerCommand } from './registry';
import { formatDate } from '../shared/output';
import { getOtaSourceCurrency, inferSourceIdFromUrl } from '../../config/loader';
import { FRESHNESS } from '../../config/constants';
import { validateIsoDate, validatePositiveInt, validateDateRange } from '../../types/validation';
import { globalRegistry } from '../../scrapers/registry';
import type { OtaSearchParams, ScrapeResult } from '../../scrapers/types';
import * as fs from 'fs';
import * as path from 'path';

// ── helpers (extracted from travel-update.ts) ────────────────────────

function parseProductTypes(value: string | undefined): Array<'package' | 'flight' | 'hotel'> | undefined {
  if (!value) return undefined;
  const parts = value
    .split(',')
    .map((s) => s.trim().toLowerCase())
    .filter(Boolean);

  const out: Array<'package' | 'flight' | 'hotel'> = [];
  for (const p of parts) {
    if (p === 'package' || p === 'flight' || p === 'hotel') out.push(p);
    else {
      console.error('Error: --types must be a comma-separated list of: package,flight,hotel');
      process.exit(1);
    }
  }
  return out.length ? out : undefined;
}

async function runSearchOffers(params: OtaSearchParams, sourceOpt: string | undefined): Promise<ScrapeResult[]> {
  if (sourceOpt) {
    const scraper = globalRegistry.get(sourceOpt);
    if (!scraper) {
      return [
        {
          success: false,
          offers: [],
          provenance: {
            sourceId: sourceOpt,
            scrapedAt: new Date().toISOString(),
            offersFound: 0,
            searchParams: params,
            duration_ms: 0,
          },
          errors: [`No scraper registered for source: ${sourceOpt}`],
          warnings: [],
        },
      ];
    }
    return [await scraper.search(params)];
  }

  return await globalRegistry.searchAll(params);
}

function printSearchResults(results: ScrapeResult[]): void {
  console.log('\nResults:');
  for (const r of results) {
    const icon = r.success ? 'OK' : 'FAIL';
    console.log(`  ${icon} ${r.provenance.sourceId}: ${r.provenance.offersFound} offer(s) in ${r.provenance.duration_ms}ms`);
    if (r.errors.length) console.log(`     Errors: ${r.errors.slice(0, 2).join(' | ')}`);
    if (r.warnings.length) console.log(`     Warnings: ${r.warnings.slice(0, 2).join(' | ')}`);
    for (const o of r.offers.slice(0, 5)) {
      const price = o.priceTotal ?? o.pricePerPerson;
      const priceLabel = price ? `${o.currency} ${price.toLocaleString()}` : '(no price)';
      console.log(`     - ${o.title} - ${priceLabel} - ${o.availability}`);
    }
    if (r.offers.length > 5) console.log(`     ... and ${r.offers.length - 5} more`);
  }
  console.log('');
}

// ── compare-offers types & helpers ───────────────────────────────────

interface CompareOffer {
  file: string;
  source_id: string;
  source_name: string;
  scraped_at: string;
  url: string;
  price_per_person: number | null;
  price_total: number | null;
  currency: string;
  airline: string;
  flight_outbound: string;
  flight_return: string;
  hotel: string;
  type: 'package' | 'group_tour' | 'fit';
  dates: string;
}

function inferSourceIdFromFilename(filename: string): string {
  if (filename.includes('besttour')) return 'besttour';
  if (filename.includes('liontravel')) return 'liontravel';
  if (filename.includes('lifetour')) return 'lifetour';
  if (filename.includes('settour')) return 'settour';
  if (filename.includes('eztravel')) return 'eztravel';
  if (filename.includes('tigerair')) return 'tigerair';
  return 'unknown';
}

function inferOverallAvailability(datePricing: any): 'available' | 'sold_out' | 'limited' | null {
  if (!datePricing || typeof datePricing !== 'object') return null;
  const entries = Object.values(datePricing) as any[];
  if (entries.some(e => e?.availability === 'available')) return 'available';
  if (entries.some(e => e?.availability === 'limited')) return 'limited';
  if (entries.some(e => e?.availability === 'sold_out')) return 'sold_out';
  return null;
}

export function computeBestValue(datePricing: any, pax: number): { date: string; price_per_person: number; price_total: number; availability: any; seats_remaining?: number } | null {
  if (!datePricing || typeof datePricing !== 'object') return null;
  let best: { date: string; price_per_person: number; price_total: number; availability: any; seats_remaining?: number } | null = null;
  for (const [date, entry] of Object.entries(datePricing as Record<string, any>)) {
    const price = entry?.price;
    const availability = entry?.availability;
    if (typeof price !== 'number') continue;
    if (availability && availability !== 'available') continue;
    const candidate = { date, price_per_person: price, price_total: price * pax, availability, seats_remaining: entry?.seats_remaining };
    if (!best || candidate.price_per_person < best.price_per_person) best = candidate;
  }
  return best;
}

function loadScrapedOffers(region: string, filterDate: string | undefined, pax: number): CompareOffer[] {
  const dataDir = path.join(process.cwd(), 'data');
  if (!fs.existsSync(dataDir)) return [];

  const files = fs.readdirSync(dataDir).filter(f =>
    f.endsWith('.json') &&
    f.toLowerCase().includes(region.toLowerCase()) &&
    !f.includes('schema') &&
    !f.includes('travel-plan') &&
    !f.includes('destinations') &&
    !f.includes('ota-sources')
  );

  const offers: CompareOffer[] = [];

  for (const file of files) {
    try {
      const filePath = path.join(dataDir, file);
      const content = fs.readFileSync(filePath, 'utf-8');
      const data = JSON.parse(content);
      const offer = parseScrapedFile(file, data, pax, filterDate);
      if (offer) offers.push(offer);
    } catch {
      // Skip files that can't be parsed
    }
  }

  // Sort by price (lowest first)
  offers.sort((a, b) => {
    if (a.price_per_person === null && b.price_per_person === null) return 0;
    if (a.price_per_person === null) return 1;
    if (b.price_per_person === null) return -1;
    return a.price_per_person - b.price_per_person;
  });

  return offers;
}

function parseScrapedFile(file: string, data: any, pax: number, filterDate?: string): CompareOffer | null {
  if (!data || typeof data !== 'object') return null;

  const url = data.url || '';
  const scrapedAt = data.scraped_at || '';
  const urlSourceId = inferSourceIdFromUrl(url);
  const fileSourceId = inferSourceIdFromFilename(file);
  const sourceId = urlSourceId !== 'unknown' ? urlSourceId : fileSourceId;
  if (sourceId === 'unknown') return null;

  let currency = 'TWD';
  try {
    currency = getOtaSourceCurrency(sourceId);
  } catch {
    // Keep going with a sensible default; compare-offers should be resilient to odd files.
  }

  // Source display names
  const sourceNames: Record<string, string> = {
    besttour: 'BestTour',
    liontravel: 'LionTravel',
    lifetour: 'Lifetour',
    settour: 'Settour',
    eztravel: 'EzTravel',
    tigerair: 'Tigerair',
  };

  // Determine type from URL or content
  let type: 'package' | 'group_tour' | 'fit' = 'package';
  if (url.includes('vacation.liontravel.com') || url.includes('FIT')) {
    type = 'fit';
  } else if (url.includes('tour.') || url.includes('searchlist')) {
    type = 'group_tour';
  }

  // Try to extract price from various locations
  let pricePerPerson: number | null = null;
  let priceTotal: number | null = null;

  // Check extracted.date_pricing first (BestTour calendar)
  const extracted = data.extracted || {};
  const datePricing = extracted.date_pricing && typeof extracted.date_pricing === 'object' ? extracted.date_pricing : null;
  const hasDatePricing = Boolean(datePricing && Object.keys(datePricing).length > 0);

  if (filterDate) {
    const ymd = filterDate.replace(/-/g, '/');
    const compact = filterDate.replace(/-/g, '');

    if (datePricing) {
      const entry = (datePricing as Record<string, any>)[filterDate];
      if (!entry) return null; // user asked for a specific date and this calendar doesn't include it
      if (typeof entry.price === 'number') {
        pricePerPerson = entry.price;
        priceTotal = entry.price * pax;
      } else {
        return null;
      }
    } else {
      // If we don't have a calendar, only keep offers that clearly match the date.
      const rawText = String(data.raw_text || '');
      const matchesUrl = typeof url === 'string' && url.includes(`FromDate=${compact}`);
      const matchesText = rawText.includes(ymd) || rawText.includes(filterDate);
      if (!matchesUrl && !matchesText) return null;
    }
  } else if (hasDatePricing) {
    const best = computeBestValue(datePricing, pax);
    if (best) {
      pricePerPerson = best.price_per_person;
      priceTotal = best.price_total;
    }
  }

  // Fallback 1: Check extracted.price.per_person (Lifetour/Settour parsers)
  if (pricePerPerson === null && extracted.price?.per_person) {
    const pp = extracted.price.per_person as number;
    pricePerPerson = pp;
    priceTotal = pp * pax;
  }

  // Fallback 2: Parse from extracted_elements.price_element
  if (pricePerPerson === null) {
    const priceElements = data.extracted_elements?.price_element || [];
    for (const pe of priceElements) {
      const match = String(pe).match(/(\d{1,3}(?:,\d{3})*)/);
      if (match) {
        const num = parseInt(match[1].replace(/,/g, ''), 10);
        if (num > 10000 && num < 200000) {
          pricePerPerson = num;
          priceTotal = num * pax;
          break;
        }
      }
    }
  }

  // Fallback 3: Parse from raw_text patterns (NT$XX,XXX or TWD XX,XXX)
  if (pricePerPerson === null && data.raw_text) {
    const pricePatterns = [
      /NT\$\s*([\d,]+)/g,
      /TWD\s*([\d,]+)/g,
      /售價[：:]\s*([\d,]+)/g,
      /團費[：:]\s*([\d,]+)/g,
      /(\d{2,3},\d{3})\s*元?\/人/g,
    ];
    for (const pattern of pricePatterns) {
      const matches = [...String(data.raw_text).matchAll(pattern)];
      for (const m of matches) {
        const num = parseInt(m[1].replace(/,/g, ''), 10);
        if (num > 15000 && num < 150000) {
          pricePerPerson = num;
          priceTotal = num * pax;
          break;
        }
      }
      if (pricePerPerson !== null) break;
    }
  }

  // Extract flight info - prefer structured extracted.flight data
  let airline = '';
  let flightOutbound = '';
  let flightReturn = '';

  // Check extracted.flight first (structured parser output)
  if (extracted.flight?.outbound?.airline) {
    airline = extracted.flight.outbound.airline;
  }
  if (extracted.flight?.outbound?.departure_time && extracted.flight?.outbound?.arrival_time) {
    flightOutbound = `${extracted.flight.outbound.departure_time} → ${extracted.flight.outbound.arrival_time}`;
  }
  if (extracted.flight?.return?.departure_time && extracted.flight?.return?.arrival_time) {
    flightReturn = `${extracted.flight.return.departure_time} → ${extracted.flight.return.arrival_time}`;
  }

  // Fallback: parse from extracted_elements.flight_element
  if (!airline || !flightOutbound) {
    const flightElements = data.extracted_elements?.flight_element || [];
    for (const fe of flightElements) {
      const text = String(fe);
      if (text.includes('去程') && !flightOutbound) {
        const airlineMatch = text.match(/(長榮航空|華航|中華航空|虎航|樂桃|捷星|酷航|星宇|亞洲航空|Scoot|Peach|EVA|China Airlines|BR\d+|IT\d+|TR\d+|CI\d+|D7\d+)/i);
        if (airlineMatch && !airline) airline = airlineMatch[1];
        const timeMatch = text.match(/(\d{2}:\d{2}).*?(\d{2}:\d{2})/);
        if (timeMatch && !flightOutbound) flightOutbound = `${timeMatch[1]} → ${timeMatch[2]}`;
      }
      if (text.includes('回程') && !flightReturn) {
        const timeMatch = text.match(/(\d{2}:\d{2}).*?(\d{2}:\d{2})/);
        if (timeMatch) flightReturn = `${timeMatch[1]} → ${timeMatch[2]}`;
      }
    }
  }

  // Extract hotel info - prefer structured extracted.hotel data
  let hotel = '';
  if (extracted.hotel?.name) {
    hotel = extracted.hotel.name;
  } else if (Array.isArray(extracted.hotel?.names) && extracted.hotel.names.length > 0) {
    hotel = extracted.hotel.names[0];
  }

  // Fallback: parse from extracted_elements.hotel_element
  if (!hotel) {
    const hotelElements = data.extracted_elements?.hotel_element || [];
    if (hotelElements.length > 0) {
      hotel = String(hotelElements[0]).split('\n')[0].trim();
    }
  }

  // Extract dates from raw_text or URL
  let dates = '';
  const rawText = data.raw_text || '';
  const dateMatch = rawText.match(/(\d{4}\/\d{1,2}\/\d{1,2}).*?~.*?(\d{4}\/\d{1,2}\/\d{1,2}|\d{1,2}\/\d{1,2})/);
  if (dateMatch) {
    dates = `${dateMatch[1]} ~ ${dateMatch[2]}`;
  } else {
    const urlDateMatch = url.match(/FromDate=(\d{8})/);
    if (urlDateMatch) {
      const d = urlDateMatch[1];
      dates = `${d.slice(0,4)}/${d.slice(4,6)}/${d.slice(6,8)}`;
    }
  }

  return {
    file,
    source_id: sourceId,
    source_name: sourceNames[sourceId] || sourceId,
    scraped_at: scrapedAt,
    url,
    price_per_person: pricePerPerson,
    price_total: priceTotal,
    currency,
    airline,
    flight_outbound: flightOutbound,
    flight_return: flightReturn,
    hotel,
    type,
    dates,
  };
}

function printOfferComparison(offers: CompareOffer[], region: string, filterDate: string | undefined, pax: number): void {
  console.log(`\n  PACKAGE COMPARISON: ${region.toUpperCase()}`);
  console.log(`  Pax: ${pax} | Filter date: ${filterDate || '(all)'}`);
  console.log(`  Found ${offers.length} scraped file(s)\n`);

  if (offers.length === 0) return;

  // Print table header
  console.log('-----------------------------------------------------------------------------------------------');
  const totalHeader = `Total (${pax}pax)`.padEnd(15).slice(0, 15);
  console.log(`  OTA               Price/person    ${totalHeader}  Details`);
  console.log('-----------------------------------------------------------------------------------------------');

  for (const o of offers) {
    const priceStr = o.price_per_person !== null
      ? `${o.currency} ${o.price_per_person.toLocaleString()}`
      : '(no price)';
    const totalStr = o.price_total !== null
      ? `${o.currency} ${o.price_total.toLocaleString()}`
      : '-';

    const details: string[] = [];
    if (o.airline) details.push(o.airline);
    if (o.hotel) details.push(o.hotel.slice(0, 20));
    if (o.type === 'fit') details.push('FIT');
    if (o.type === 'group_tour') details.push('Group');

    const detailStr = details.join(' | ').slice(0, 30) || '-';

    console.log(`  ${o.source_name.padEnd(15)} ${priceStr.padEnd(15)} ${totalStr.padEnd(15)} ${detailStr}`);
  }

  console.log('-----------------------------------------------------------------------------------------------');

  // Print staleness warning for old data
  const now = Date.now();
  const staleThresholdMs = FRESHNESS.STALE_THRESHOLD_MS;
  const staleOffers = offers.filter(o => {
    if (!o.scraped_at) return false;
    const scrapedTime = new Date(o.scraped_at).getTime();
    return now - scrapedTime > staleThresholdMs;
  });

  if (staleOffers.length > 0) {
    console.log(`\n  Warning: ${staleOffers.length} offer(s) have stale data (>24h old). Consider re-scraping.`);
  }

  // Print best value recommendation
  const bestOffer = offers[0];
  if (bestOffer && bestOffer.price_per_person !== null) {
    console.log(`\n  Best value: ${bestOffer.source_name} at ${bestOffer.currency} ${bestOffer.price_per_person.toLocaleString()}/person`);
    if (bestOffer.hotel) console.log(`   Hotel: ${bestOffer.hotel}`);
    if (bestOffer.airline) console.log(`   Airline: ${bestOffer.airline}`);
  }

  console.log('');
}

// ── search-offers ────────────────────────────────────────────────────

const searchOffersCommand: CommandHandler = {
  names: ['search-offers'],
  description: 'Search for travel offers from registered OTA sources.',
  usage: 'search-offers --dest <slug> [--start <date>] [--end <date>] [--pax N] [--types package,flight,hotel] [--source <id>] [--json]',
  async execute(ctx: CliContext): Promise<void> {
    const { sm, args } = ctx;
    const destOpt = args.optionValue('--dest');
    const startOpt = args.optionValue('--start');
    const endOpt = args.optionValue('--end');
    const paxOpt = args.optionValue('--pax');
    const typesOpt = args.optionValue('--types');
    const sourceOpt = args.optionValue('--source');
    const jsonOpt = args.hasFlag('--json');

    const destination = destOpt;
    if (!destination) {
      console.error('Error: search-offers requires --dest <slug>');
      process.exit(1);
    }

    const plan = sm.getPlan();
    const destObj = plan.destinations[destination] as Record<string, unknown> | undefined;
    if (!destObj) {
      console.error(`Error: Destination not found: ${destination}`);
      process.exit(1);
    }

    const destAnchor = destObj.process_1_date_anchor as Record<string, unknown> | undefined;
    const confirmedDates = destAnchor?.confirmed_dates as { start: string; end: string } | undefined;

    const startDate = startOpt || confirmedDates?.start;
    const endDate = endOpt || confirmedDates?.end;
    if (!startDate || !endDate) {
      console.error('Error: search-offers requires --start and --end (or destination confirmed dates in plan).');
      console.error('Fix: set-dates <start> <end> first, or pass --start/--end explicitly.');
      process.exit(1);
    }

    const rangeResult = validateDateRange(startDate, endDate);
    if (!rangeResult.ok) {
      console.error(`Error: ${rangeResult.error}`);
      process.exit(1);
    }

    let pax = 2;
    if (paxOpt) {
      const paxResult = validatePositiveInt(paxOpt, '--pax');
      if (!paxResult.ok) {
        console.error(`Error: ${paxResult.error}`);
        process.exit(1);
      }
      pax = paxResult.value;
    }

    const productTypes = parseProductTypes(typesOpt);

    const params: OtaSearchParams = {
      destination,
      startDate,
      endDate,
      pax,
      ...(productTypes ? { productTypes } : {}),
    };

    console.log(`\nsearch-offers (${destination})`);
    console.log(`   Dates: ${formatDate(startDate)} → ${formatDate(endDate)} (${rangeResult.value.days} days)`);
    console.log(`   Pax: ${pax}`);
    if (productTypes) console.log(`   Types: ${productTypes.join(', ')}`);
    if (sourceOpt) console.log(`   Source: ${sourceOpt}`);

    const results = await runSearchOffers(params, sourceOpt);
    if (jsonOpt) {
      console.log(JSON.stringify(results, null, 2));
    } else {
      printSearchResults(results);
    }

    const anySuccess = results.some((r) => r.success);
    process.exitCode = anySuccess ? 0 : 1;
  },
};

registerCommand(searchOffersCommand);

// ── compare-offers ───────────────────────────────────────────────────

const compareOffersCommand: CommandHandler = {
  names: ['compare-offers'],
  description: 'Compare scraped offers from data/ directory for a region.',
  usage: 'compare-offers --region <name> [--date YYYY-MM-DD] [--pax N] [--json]',
  async execute(ctx: CliContext): Promise<void> {
    const { args } = ctx;
    const region = args.optionValue('--region');
    const filterDate = args.optionValue('--date');
    const paxOpt = args.optionValue('--pax');
    const jsonOpt = args.hasFlag('--json');

    if (!region) {
      console.error('Error: compare-offers requires --region <name>');
      console.error('Example: compare-offers --region osaka');
      process.exit(1);
    }

    if (filterDate) {
      const dateResult = validateIsoDate(filterDate);
      if (!dateResult.ok) {
        console.error(`Error: --date: ${dateResult.error}`);
        process.exit(1);
      }
    }

    let pax = 2;
    if (paxOpt) {
      const paxResult = validatePositiveInt(paxOpt, '--pax');
      if (!paxResult.ok) {
        console.error(`Error: ${paxResult.error}`);
        process.exit(1);
      }
      pax = paxResult.value;
    }

    const offers = loadScrapedOffers(region, filterDate, pax);
    if (offers.length === 0) {
      console.log(`\nNo scraped offers found for region "${region}".`);
      console.log(`Make sure you have scrapes/*${region}*.json files from previous scrapes.`);
      process.exit(1);
    }

    if (jsonOpt) {
      console.log(JSON.stringify(offers, null, 2));
    } else {
      printOfferComparison(offers, region, filterDate, pax);
    }
  },
};

registerCommand(compareOffersCommand);
