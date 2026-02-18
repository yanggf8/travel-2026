/**
 * Scrape File Parser
 *
 * Parses scraper JSON output files into CanonicalOffer[].
 * Supports two formats:
 *   Format A: `{ offers: [...] }` — scraper listings output
 *   Format B: `{ url, extracted, ... }` — raw single scrape
 *
 * Groups parsed offers by sourceId (one ParsedOfferFile per source).
 */

import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import type { CanonicalOffer, FlightSegment, HotelInfo, DatePricing } from './types';

// ============================================================================
// Public API
// ============================================================================

export interface ParsedOfferFile {
  sourceId: string;
  filePath: string;
  fileName: string;
  offers: CanonicalOffer[];
  warnings: string[];
}

export function parseScrapeFiles(
  filePaths: string[],
  opts?: {
    start?: string;
    end?: string;
    includeUndated?: boolean;
    pax?: number;
  }
): ParsedOfferFile[] {
  const bySource = new Map<string, {
    offers: CanonicalOffer[];
    warnings: string[];
    filePath: string;
    fileName: string;
  }>();

  for (const fp of filePaths) {
    const fileName = path.basename(fp);
    let raw: unknown;
    try {
      raw = JSON.parse(fs.readFileSync(fp, 'utf-8'));
    } catch {
      continue;
    }

    if (!raw || typeof raw !== 'object' || Array.isArray(raw)) continue;
    const obj = raw as Record<string, unknown>;

    const parsedOffers: CanonicalOffer[] = [];
    const fileWarnings: string[] = [];

    if (Array.isArray(obj.offers)) {
      // Format A — `{ offers: [...] }` from scraper listings
      for (const o of obj.offers) {
        if (!o || typeof o !== 'object' || Array.isArray(o)) continue;
        try {
          const offer = parseFormatAOffer(o as Record<string, unknown>, fileName);
          if (offer) parsedOffers.push(offer);
        } catch (e) {
          fileWarnings.push(`Parse error in ${fileName}: ${(e as Error).message}`);
        }
      }
    } else {
      // Format B — raw single scrape with `extracted` key
      try {
        const offer = parseFormatBOffer(obj, fileName);
        if (offer) parsedOffers.push(offer);
      } catch (e) {
        fileWarnings.push(`Parse error in ${fileName}: ${(e as Error).message}`);
      }
    }

    // Apply date filter
    const filtered = parsedOffers.filter(o => {
      const date = getRepresentativeDate(o);
      return isWithinDateRange(date, opts?.start, opts?.end, opts?.includeUndated ?? true);
    });

    // Group by sourceId
    for (const offer of filtered) {
      const { sourceId } = offer;
      if (!bySource.has(sourceId)) {
        bySource.set(sourceId, { offers: [], warnings: [], filePath: fp, fileName });
      }
      const group = bySource.get(sourceId)!;
      group.offers.push(offer);
    }

    // Attach file-level warnings to every group that got offers from this file
    if (fileWarnings.length > 0) {
      for (const group of bySource.values()) {
        group.warnings.push(...fileWarnings);
      }
    }
  }

  return Array.from(bySource.entries()).map(([sourceId, group]) => ({
    sourceId,
    filePath: group.filePath,
    fileName: group.fileName,
    offers: group.offers,
    warnings: [...new Set(group.warnings)],
  }));
}

// ============================================================================
// Inference helpers (exported for reuse)
// ============================================================================

export function inferSourceIdFromFilename(filename: string): string {
  const f = filename.toLowerCase();
  if (f.includes('besttour')) return 'besttour';
  if (f.includes('liontravel')) return 'liontravel';
  if (f.includes('lifetour')) return 'lifetour';
  if (f.includes('settour')) return 'settour';
  if (f.includes('travel4u')) return 'travel4u';
  if (f.includes('eztravel')) return 'eztravel';
  if (f.includes('tigerair')) return 'tigerair';
  if (f.includes('agoda')) return 'agoda';
  if (f.includes('booking')) return 'booking';
  return 'unknown';
}

export function inferDestinationFromFilename(filename: string): string | null {
  const f = filename.toLowerCase();
  if (f.includes('tokyo') || f.includes('tyo')) return 'tokyo_2026';
  if (f.includes('osaka_kyoto')) return 'osaka_kyoto_2026';
  if (f.includes('osaka') || f.includes('kansai') || f.includes('kyoto') || f.includes('kix')) return 'osaka_2026';
  return null;
}

// ============================================================================
// Internal helpers
// ============================================================================

function inferSourceIdFromUrl(url: string): string {
  const u = url.toLowerCase();
  if (u.includes('besttour.com.tw')) return 'besttour';
  if (u.includes('liontravel.com')) return 'liontravel';
  if (u.includes('lifetour.com.tw')) return 'lifetour';
  if (u.includes('settour.com.tw')) return 'settour';
  if (u.includes('travel4u.com.tw')) return 'travel4u';
  if (u.includes('eztravel')) return 'eztravel';
  if (u.includes('tigerair')) return 'tigerair';
  if (u.includes('agoda')) return 'agoda';
  if (u.includes('booking.com')) return 'booking';
  return 'unknown';
}

function stableHash8(input: string): string {
  return crypto.createHash('sha1').update(input).digest('hex').slice(0, 8);
}

function normalizeIsoDate(s: unknown): string | null {
  if (typeof s !== 'string') return null;
  const v = s.trim();
  if (!v) return null;
  if (/^\d{4}-\d{2}-\d{2}$/.test(v)) return v;
  const m = v.match(/^(\d{4})\/(\d{1,2})\/(\d{1,2})$/);
  if (m) {
    return `${m[1]}-${m[2].padStart(2, '0')}-${m[3].padStart(2, '0')}`;
  }
  return null;
}

function productCodeFromUrl(url: string): string | null {
  if (!url) return null;
  try {
    const u = new URL(url);
    const segs = u.pathname.split('/').filter(Boolean);
    const last = segs.at(-1);
    if (!last) return null;
    return last.replace(/\.html?$/i, '') || null;
  } catch {
    const base = url.split('?')[0] || '';
    const parts = base.split('/').filter(Boolean);
    return parts.at(-1) || null;
  }
}

function mapFlightLeg(leg: Record<string, unknown> | null | undefined): FlightSegment | null {
  if (!leg) return null;
  const flightNumber =
    (typeof leg.flight_number === 'string' ? leg.flight_number : '') ||
    (typeof (leg as any).flightNumber === 'string' ? (leg as any).flightNumber : '');
  if (!flightNumber) return null;

  return {
    flightNumber,
    airline: (typeof leg.airline === 'string' ? leg.airline : '') || '',
    airlineCode: typeof leg.airline_code === 'string'
      ? leg.airline_code
      : typeof (leg as any).airlineCode === 'string' ? (leg as any).airlineCode : undefined,
    departureAirport: '',
    departureCode:
      (typeof leg.departure_code === 'string' ? leg.departure_code : '') ||
      (typeof (leg as any).departure_airport_code === 'string' ? (leg as any).departure_airport_code : '') ||
      (typeof (leg as any).departureCode === 'string' ? (leg as any).departureCode : ''),
    departureTerminal: typeof leg.departure_terminal === 'string' ? leg.departure_terminal : undefined,
    departureTime:
      (typeof leg.departure_time === 'string' ? leg.departure_time : '') ||
      (typeof (leg as any).departureTime === 'string' ? (leg as any).departureTime : ''),
    arrivalAirport: '',
    arrivalCode:
      (typeof leg.arrival_code === 'string' ? leg.arrival_code : '') ||
      (typeof (leg as any).arrival_airport_code === 'string' ? (leg as any).arrival_airport_code : '') ||
      (typeof (leg as any).arrivalCode === 'string' ? (leg as any).arrivalCode : ''),
    arrivalTerminal: typeof leg.arrival_terminal === 'string' ? leg.arrival_terminal : undefined,
    arrivalTime:
      (typeof leg.arrival_time === 'string' ? leg.arrival_time : '') ||
      (typeof (leg as any).arrivalTime === 'string' ? (leg as any).arrivalTime : ''),
    date: normalizeIsoDate(leg.date) || '',
  };
}

function mapHotel(hotel: Record<string, unknown> | null | undefined): HotelInfo | null {
  if (!hotel) return null;
  const name =
    (typeof hotel.name === 'string' ? hotel.name : '') ||
    (Array.isArray(hotel.names) && typeof hotel.names[0] === 'string' ? hotel.names[0] : '');
  if (!name) return null;

  return {
    name,
    area: typeof hotel.area === 'string' ? hotel.area : '',
    starRating: typeof hotel.star_rating === 'number' ? hotel.star_rating : undefined,
    access: Array.isArray(hotel.access) ? hotel.access.map(String) : undefined,
    roomType: typeof hotel.room_type === 'string' ? hotel.room_type : undefined,
  };
}

function mapDatePricingDict(dict: Record<string, unknown>): DatePricing[] {
  const result: DatePricing[] = [];
  for (const [date, entry] of Object.entries(dict)) {
    const normalizedDate = normalizeIsoDate(date);
    if (!normalizedDate || !entry || typeof entry !== 'object') continue;
    const e = entry as Record<string, unknown>;
    const price =
      typeof e.price === 'number' ? e.price :
      typeof e.price_per_person === 'number' ? e.price_per_person :
      null;
    if (price === null || !Number.isFinite(price)) continue;
    result.push({
      date: normalizedDate,
      pricePerPerson: price,
      priceTotal: typeof e.price_total === 'number' ? e.price_total : undefined,
      availability: (typeof e.availability === 'string' ? e.availability : 'unknown') as DatePricing['availability'],
      notes: typeof e.notes === 'string' ? e.notes : undefined,
    });
  }
  return result;
}

function mapBestValue(bv: Record<string, unknown> | null | undefined): CanonicalOffer['bestValue'] | undefined {
  if (!bv) return undefined;
  const date = normalizeIsoDate(bv.date);
  const price =
    typeof bv.price_per_person === 'number' ? bv.price_per_person :
    typeof (bv as any).pricePerPerson === 'number' ? (bv as any).pricePerPerson :
    null;
  if (!date || price === null) return undefined;
  const total =
    typeof bv.price_total === 'number' ? bv.price_total :
    typeof (bv as any).priceTotal === 'number' ? (bv as any).priceTotal :
    0;
  return { date, pricePerPerson: price, priceTotal: total };
}

function getRepresentativeDate(offer: CanonicalOffer): string | null {
  if (offer.bestValue?.date) return offer.bestValue.date;
  if (offer.datePricing && offer.datePricing.length > 0) {
    return [...offer.datePricing].sort((a, b) => a.date.localeCompare(b.date))[0].date;
  }
  return null;
}

function isWithinDateRange(
  date: string | null,
  start?: string,
  end?: string,
  includeUndated = true
): boolean {
  if (!start && !end) return true;
  if (!date) return includeUndated;
  if (start && date < start) return false;
  if (end && date > end) return false;
  return true;
}

// ============================================================================
// Format A parser — scraper listings: `{ offers: [...] }`
// ============================================================================

function parseFormatAOffer(
  offer: Record<string, unknown>,
  fileName: string
): CanonicalOffer | null {
  const url = typeof offer.url === 'string' ? offer.url : '';
  const sourceId =
    (typeof offer.source_id === 'string' && offer.source_id) ||
    (typeof offer.sourceId === 'string' && offer.sourceId) ||
    (url ? inferSourceIdFromUrl(url) : '') ||
    inferSourceIdFromFilename(fileName);
  if (!sourceId || sourceId === 'unknown') return null;

  const explicitId = typeof offer.id === 'string' ? offer.id : null;
  const productCode = typeof offer.product_code === 'string' ? offer.product_code : null;
  const id = explicitId || (url
    ? `${sourceId}_${productCode || productCodeFromUrl(url) || stableHash8(url)}`
    : null);
  if (!id) return null;

  const typeRaw = (typeof offer.type === 'string' ? offer.type : 'package').toLowerCase();
  const type: CanonicalOffer['type'] = typeRaw === 'flight' ? 'flight' : typeRaw === 'hotel' ? 'hotel' : 'package';

  const title =
    (typeof offer.title === 'string' ? offer.title : '') ||
    (typeof offer.name === 'string' ? offer.name : '');

  const currency = (typeof offer.currency === 'string' && offer.currency) || 'TWD';

  const pricePerPerson =
    typeof offer.price_per_person === 'number' && Number.isFinite(offer.price_per_person) ? offer.price_per_person :
    typeof offer.pricePerPerson === 'number' && Number.isFinite(offer.pricePerPerson) ? offer.pricePerPerson :
    null;
  if (pricePerPerson === null) return null;

  const availability = (typeof offer.availability === 'string' ? offer.availability : 'unknown') as CanonicalOffer['availability'];

  const flightRaw = offer.flight && typeof offer.flight === 'object' ? offer.flight as Record<string, unknown> : null;
  let flight: CanonicalOffer['flight'] | undefined;
  if (flightRaw) {
    const outbound = mapFlightLeg(flightRaw.outbound as Record<string, unknown> | undefined);
    const ret = mapFlightLeg(flightRaw.return as Record<string, unknown> | undefined);
    if (outbound && ret) flight = { outbound, return: ret };
  }

  const hotelRaw = offer.hotel && typeof offer.hotel === 'object' ? offer.hotel as Record<string, unknown> : null;
  const hotel = mapHotel(hotelRaw) ?? undefined;

  let datePricing: DatePricing[] | undefined;
  const dpRaw = offer.date_pricing ?? offer.datePricing;
  if (dpRaw && typeof dpRaw === 'object' && !Array.isArray(dpRaw)) {
    datePricing = mapDatePricingDict(dpRaw as Record<string, unknown>);
  } else if (Array.isArray(dpRaw)) {
    datePricing = (dpRaw as any[]).filter(d => d && typeof d === 'object').map((d: any) => ({
      date: normalizeIsoDate(d.date) || d.date || '',
      pricePerPerson: typeof d.pricePerPerson === 'number' ? d.pricePerPerson :
        typeof d.price_per_person === 'number' ? d.price_per_person :
        typeof d.price === 'number' ? d.price : 0,
      availability: (d.availability || 'unknown') as DatePricing['availability'],
    })).filter((d: DatePricing) => d.date && Number.isFinite(d.pricePerPerson));
  }

  const bvRaw = (offer.best_value ?? offer.bestValue) as Record<string, unknown> | undefined;
  const bestValue = mapBestValue(bvRaw);

  const scrapedAt =
    (typeof offer.scraped_at === 'string' ? offer.scraped_at : '') ||
    (typeof offer.scrapedAt === 'string' ? offer.scrapedAt : new Date().toISOString());

  return {
    id,
    sourceId,
    type,
    title,
    url,
    currency,
    pricePerPerson,
    availability,
    ...(flight ? { flight } : {}),
    ...(hotel ? { hotel } : {}),
    ...(datePricing && datePricing.length > 0 ? { datePricing } : {}),
    ...(bestValue ? { bestValue } : {}),
    scrapedAt,
  };
}

// ============================================================================
// Format B parser — raw single scrape: `{ url, extracted, ... }`
// ============================================================================

function parseFormatBOffer(
  data: Record<string, unknown>,
  fileName: string
): CanonicalOffer | null {
  const url = typeof data.url === 'string' ? data.url : '';
  const extracted = (data.extracted && typeof data.extracted === 'object')
    ? data.extracted as Record<string, unknown>
    : {};

  const sourceIdFromData =
    (typeof data.source_id === 'string' && data.source_id) ||
    (typeof data.sourceId === 'string' && data.sourceId) ||
    '';
  const sourceId =
    sourceIdFromData ||
    (url ? inferSourceIdFromUrl(url) : '') ||
    inferSourceIdFromFilename(fileName);
  if (!sourceId || sourceId === 'unknown') return null;

  const id = `${sourceId}_${productCodeFromUrl(url) || stableHash8(url)}`;

  const priceRaw = extracted.price && typeof extracted.price === 'object'
    ? extracted.price as Record<string, unknown>
    : null;
  const pricePerPerson = priceRaw &&
    typeof priceRaw.per_person === 'number' &&
    Number.isFinite(priceRaw.per_person)
      ? priceRaw.per_person
      : null;
  if (pricePerPerson === null) return null;

  const currency = (priceRaw && typeof priceRaw.currency === 'string' ? priceRaw.currency : '') || 'TWD';

  const flightRaw = extracted.flight && typeof extracted.flight === 'object'
    ? extracted.flight as Record<string, unknown>
    : null;
  const hotelRaw = extracted.hotel && typeof extracted.hotel === 'object'
    ? extracted.hotel as Record<string, unknown>
    : null;

  const hasFlight = Boolean(flightRaw && (flightRaw.outbound || flightRaw.return));
  const hasHotel = Boolean(hotelRaw && ((hotelRaw as any).name || (hotelRaw as any).names));
  let type: CanonicalOffer['type'] = 'package';
  if (hasFlight && !hasHotel) type = 'flight';
  if (hasHotel && !hasFlight) type = 'hotel';

  let flight: CanonicalOffer['flight'] | undefined;
  if (flightRaw) {
    const outbound = mapFlightLeg(flightRaw.outbound as Record<string, unknown> | undefined);
    const ret = mapFlightLeg(flightRaw.return as Record<string, unknown> | undefined);
    if (outbound && ret) flight = { outbound, return: ret };
  }

  const hotel = mapHotel(hotelRaw) ?? undefined;

  let datePricing: DatePricing[] | undefined;
  const dpRaw = extracted.date_pricing;
  if (dpRaw && typeof dpRaw === 'object' && !Array.isArray(dpRaw)) {
    datePricing = mapDatePricingDict(dpRaw as Record<string, unknown>);
  }

  const title = typeof data.title === 'string' ? data.title : '';
  const scrapedAt = typeof data.scraped_at === 'string' ? data.scraped_at : new Date().toISOString();

  return {
    id,
    sourceId,
    type,
    title,
    url,
    currency,
    pricePerPerson,
    availability: 'unknown',
    ...(flight ? { flight } : {}),
    ...(hotel ? { hotel } : {}),
    ...(datePricing && datePricing.length > 0 ? { datePricing } : {}),
    scrapedAt,
  };
}
