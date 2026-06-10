import { getOtaSourceCurrency, inferSourceIdFromUrl } from '../../config/loader';

function normalizeHotel(hotel: any): any {
  if (!hotel || typeof hotel !== 'object') {
    return { name: '', slug: '', area: '', star_rating: null, access: [] };
  }
  const name = hotel.name || '';
  const slug = typeof name === 'string' ? name.toLowerCase().replace(/[^a-z0-9]+/g, '_').replace(/^_+|_+$/g, '') : '';
  const area = hotel.area || '';
  const access = Array.isArray(hotel.access) ? hotel.access : [];
  return { name, slug, area, star_rating: null, access };
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

export function normalizeScrapeToOffer(scrape: any, pax: number, warnings: string[]): any {
  const url: string = scrape?.url || '';
  const scrapedAt: string = scrape?.scraped_at || new Date().toISOString();
  const sourceId = inferSourceIdFromUrl(url);
  const productCode = url.split('/').filter(Boolean).pop() || 'unknown';
  const id = `${sourceId}_${productCode}`;

  // Get currency from OTA source config instead of hardcoding
  // (but don't crash compare/import flows if the URL is unknown)
  let currency = 'TWD';
  try {
    currency = getOtaSourceCurrency(sourceId);
  } catch {
    warnings.push(`Unknown OTA source for URL; defaulting currency to ${currency}: ${url}`);
  }

  const extracted = scrape?.extracted || {};
  const flight = extracted.flight || {};
  const outbound = flight.outbound || {};
  const inbound = flight.return || {};

  const airline: string | undefined = outbound.airline;
  const flightNumber: string | undefined = outbound.flight_number;
  const airlineCode = typeof flightNumber === 'string' ? (flightNumber.match(/^([A-Z0-9]{2})/)?.[1] || undefined) : undefined;

  const datePricing = extracted.date_pricing || {};
  const bestValue = computeBestValue(datePricing, pax);
  const availability = bestValue?.availability || inferOverallAvailability(datePricing) || 'limited';
  const pricePerPerson = bestValue?.price_per_person || undefined;
  const priceTotal = bestValue?.price_total || (pricePerPerson ? pricePerPerson * pax : undefined);

  const offer: any = {
    id,
    source_id: sourceId,
    product_code: productCode,
    url,
    scraped_at: scrapedAt,
    type: 'package',
    currency,
    availability,
    price_per_person: pricePerPerson ?? 0,
    ...(priceTotal !== undefined ? { price_total: priceTotal } : {}),
    ...(bestValue?.seats_remaining !== undefined ? { seats_remaining: bestValue.seats_remaining } : {}),
    ...(Object.keys(datePricing).length > 0 ? { date_pricing: datePricing } : {}),
    ...(bestValue ? { best_value: { date: bestValue.date, price_per_person: bestValue.price_per_person, price_total: bestValue.price_total } } : {}),
    flight: {
      airline: airline || '',
      airline_code: airlineCode || '',
      outbound: {
        flight_number: flightNumber || '',
        departure_airport_code: outbound.departure_code || '',
        arrival_airport_code: outbound.arrival_code || '',
        departure_time: outbound.departure_time || '',
        arrival_time: outbound.arrival_time || '',
      },
      return: inbound && Object.keys(inbound).length > 0 ? {
        flight_number: inbound.flight_number || '',
        departure_airport_code: inbound.departure_code || '',
        arrival_airport_code: inbound.arrival_code || '',
        departure_time: inbound.departure_time || '',
        arrival_time: inbound.arrival_time || '',
      } : null,
    },
    hotel: normalizeHotel(extracted.hotel),
    ...(Array.isArray(extracted.inclusions) ? { includes: extracted.inclusions } : {}),
  };

  if (!offer.flight.outbound.flight_number) warnings.push('Missing outbound flight_number from scraper output');
  if (!offer.hotel?.name) warnings.push('Missing hotel name from scraper output');
  if (!bestValue?.date) warnings.push('Missing price calendar; no best_value computed');

  return offer;
}
