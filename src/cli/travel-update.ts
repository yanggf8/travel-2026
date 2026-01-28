#!/usr/bin/env npx ts-node
/**
 * Travel Update CLI
 * 
 * Quick command-line interface for updating travel plan state.
 * Supports date changes, offer updates, and selections without editing JSON.
 * 
 * Usage:
 *   npx ts-node src/cli/travel-update.ts <command> [options]
 * 
 * Commands:
 *   set-dates <start> <end> [reason]     Set travel dates (triggers cascade)
 *   update-offer <offer-id> <date> <availability> [price] [seats]
 *   select-offer <offer-id> <date>       Select an offer for booking
 *   status                               Show current plan status
 * 
 * Examples:
 *   npx ts-node src/cli/travel-update.ts set-dates 2026-02-13 2026-02-17 "Agent offered Feb 13"
 *   npx ts-node src/cli/travel-update.ts update-offer besttour_TYO05MM260211AM 2026-02-13 available 27888 2
 *   npx ts-node src/cli/travel-update.ts select-offer besttour_TYO05MM260211AM 2026-02-13
 *   npx ts-node src/cli/travel-update.ts status
 */

import { StateManager } from '../state/state-manager';
import type { ProcessId } from '../state/types';
import { execFileSync } from 'child_process';
import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';

const HELP = `
Travel Update CLI - Quick updates to travel plan

Usage:
  npx ts-node src/cli/travel-update.ts <command> [options]

Commands:
  set-dates <start> <end> [reason]
    Set travel dates. Triggers cascade to invalidate dependent processes.
    Example: set-dates 2026-02-13 2026-02-17 "Agent offered Feb 13"

  scrape-package <url> [--pax N] [--dest slug]
    Scrape a package itinerary URL and import it into P3+4 offers.
    Example: scrape-package "https://www.besttour.com.tw/itinerary/TYO05MM260211AM" --pax 2

  update-offer <offer-id> <date> <availability> [price] [seats] [source]
    Update offer availability for a specific date.
    availability: available | sold_out | limited
    Example: update-offer besttour_TYO05MM260211AM 2026-02-13 available 27888 2 agent

  select-offer <offer-id> <date> [--no-populate]
    Select an offer for booking. Populates P3/P4 from offer by default.
    Example: select-offer besttour_TYO05MM260211AM 2026-02-13

  status
    Show current plan status summary.

  help
    Show this help message.

Options:
  --plan <path>  Travel plan path (default: data/travel-plan.json or $TRAVEL_PLAN_PATH)
  --state <path> State log path (default: data/state.json or $TRAVEL_STATE_PATH)
  --dry-run    Show what would be changed without saving
  --verbose    Show detailed output
  --full       Show booked offer/flight/hotel details (status only)
`;

function formatDate(dateStr: string): string {
  const d = new Date(dateStr);
  return d.toLocaleDateString('en-US', { weekday: 'short', month: 'short', day: 'numeric', year: 'numeric' });
}

function showStatus(sm: StateManager, opts?: { full?: boolean }): void {
  const plan = sm.getPlan();
  const dest = sm.getActiveDestination();
  const dates = sm.getDateAnchor();
  const dirty = sm.getDirtyFlags();
  const full = opts?.full ?? false;

  console.log('\n╔════════════════════════════════════════════════════════════╗');
  console.log('║              TRAVEL PLAN STATUS                            ║');
  console.log('╚════════════════════════════════════════════════════════════╝\n');

  console.log(`Active Destination: ${dest}`);
  
  if (dates) {
    console.log(`Travel Dates: ${formatDate(dates.start)} → ${formatDate(dates.end)} (${dates.days} days)`);
  }

  console.log('\nProcess Status:');
  console.log('─'.repeat(50));

  const destObj = plan.destinations[dest];
  if (destObj) {
    const processes: Array<{ id: ProcessId; name: string }> = [
      { id: 'process_1_date_anchor', name: 'P1 Date Anchor' },
      { id: 'process_2_destination', name: 'P2 Destination' },
      { id: 'process_3_4_packages', name: 'P3+4 Packages' },
      { id: 'process_3_transportation', name: 'P3 Transport' },
      { id: 'process_4_accommodation', name: 'P4 Accommodation' },
      { id: 'process_5_daily_itinerary', name: 'P5 Itinerary' },
    ];

    for (const p of processes) {
      const proc = destObj[p.id] as Record<string, unknown> | undefined;
      const status = proc?.status as string || 'pending';
      const isDirty = dirty.destinations?.[dest]?.[p.id]?.dirty;
      
      const statusIcon = {
        pending: '⏳',
        researching: '🔍',
        researched: '📋',
        selecting: '🎯',
        selected: '✅',
        populated: '📦',
        booking: '💳',
        booked: '🎫',
        confirmed: '✓',
        skipped: '⏭️',
      }[status] || '❓';

      const dirtyFlag = isDirty ? ' ⚠️ DIRTY' : '';
      console.log(`  ${statusIcon} ${p.name.padEnd(20)} ${status}${dirtyFlag}`);
    }
  }

  // Show chosen offer if any
  const packages = destObj?.process_3_4_packages as Record<string, unknown> | undefined;
  const chosenOfferMeta = packages?.chosen_offer as Record<string, unknown> | undefined;
  const chosenOffer = (packages?.results as Record<string, unknown> | undefined)?.chosen_offer as Record<string, unknown> | undefined;
  if (chosenOfferMeta || chosenOffer) {
    console.log('\nSelected Offer:');
    console.log('─'.repeat(50));
    if (chosenOfferMeta) {
      console.log(`  ID: ${chosenOfferMeta.id}`);
      console.log(`  Date: ${chosenOfferMeta.selected_date}`);
      console.log(`  Selected: ${chosenOfferMeta.selected_at}`);
    } else if (chosenOffer?.id) {
      console.log(`  ID: ${chosenOffer.id}`);
    }
  }

  if (full && destObj) {
    const p3 = destObj.process_3_transportation as Record<string, unknown> | undefined;
    const flight = p3?.flight as Record<string, unknown> | undefined;
    const outbound = flight?.outbound as Record<string, unknown> | undefined;
    const inbound = flight?.return as Record<string, unknown> | undefined;

    const p4 = destObj.process_4_accommodation as Record<string, unknown> | undefined;
    const hotel = p4?.hotel as Record<string, unknown> | undefined;

    if (outbound && (outbound.flight_number || outbound.departure_airport_code)) {
      console.log('\nFlight Details:');
      console.log('─'.repeat(50));
      const airline = flight?.airline as string | undefined;
      const airlineCode = flight?.airline_code as string | undefined;
      const num = outbound.flight_number as string | undefined;
      console.log(`  ${[airlineCode, num].filter(Boolean).join(' ')}${airline ? ` (${airline})` : ''}`);
      console.log(`  ${outbound.departure_airport_code ?? ''} ${outbound.departure_time ?? ''} → ${outbound.arrival_airport_code ?? ''} ${outbound.arrival_time ?? ''}`);
      if (inbound && (inbound.flight_number || inbound.departure_airport_code)) {
        const rnum = inbound.flight_number as string | undefined;
        console.log(`  Return: ${rnum ?? ''}`);
        console.log(`  ${inbound.departure_airport_code ?? ''} ${inbound.departure_time ?? ''} → ${inbound.arrival_airport_code ?? ''} ${inbound.arrival_time ?? ''}`);
      }
    }

    if (hotel && (hotel.name || hotel.area)) {
      console.log('\nHotel Details:');
      console.log('─'.repeat(50));
      console.log(`  ${hotel.name ?? ''}${hotel.area ? ` (${hotel.area})` : ''}`);
      const access = hotel.access as unknown;
      if (Array.isArray(access) && access.length > 0) {
        console.log(`  Access: ${access.slice(0, 4).join(', ')}`);
      }
      const includes = chosenOffer?.includes as unknown;
      if (Array.isArray(includes) && includes.length > 0) {
        console.log(`  Includes: ${includes.join(', ')}`);
      }
    }
  }

  console.log('\n');
}

async function main(): Promise<void> {
  const args = process.argv.slice(2);
  const command = args[0];

  if (!command || command === 'help' || command === '--help' || command === '-h') {
    console.log(HELP);
    process.exit(0);
  }

  const dryRun = args.includes('--dry-run');
  const verbose = args.includes('--verbose');
  const full = args.includes('--full');

  const optionValue = (name: string): string | undefined => {
    const eq = args.find(a => a.startsWith(`${name}=`));
    if (eq) return eq.slice(name.length + 1);
    const idx = args.indexOf(name);
    if (idx !== -1 && idx + 1 < args.length) return args[idx + 1];
    return undefined;
  };

  const destOpt = optionValue('--dest');
  const paxOpt = optionValue('--pax');
  const planOpt = optionValue('--plan');
  const stateOpt = optionValue('--state');

  // Filter out flags/options from args
  const optionsWithValues = new Set(['--dest', '--pax', '--plan', '--state']);
  const cleanArgs: string[] = [];
  for (let i = 0; i < args.length; i++) {
    const a = args[i];
    if (a.startsWith('--')) {
      if (optionsWithValues.has(a)) i++; // skip value
      continue;
    }
    cleanArgs.push(a);
  }

  const sm = new StateManager(planOpt, stateOpt);

  try {
    switch (command) {
      case 'status': {
        showStatus(sm, { full });
        break;
      }

      case 'scrape-package': {
        const [, url] = cleanArgs;
        if (!url) {
          console.error('Error: scrape-package requires <url>');
          console.error('Example: scrape-package "https://www.besttour.com.tw/itinerary/TYO05MM260211AM" --pax 2');
          process.exit(1);
        }

        const destination = destOpt || sm.getActiveDestination();
        const pax = paxOpt ? parseInt(paxOpt, 10) : 2;
        if (!Number.isFinite(pax) || pax <= 0) {
          console.error('Error: --pax must be a positive integer');
          process.exit(1);
        }

        const tmpOut = path.join(os.tmpdir(), `package-scrape-${Date.now()}.json`);
        console.log(`\n🕷️  Scraping package URL: ${url}`);
        console.log(`   Destination: ${destination}`);
        console.log(`   Pax: ${pax}`);

        if (!dryRun) {
          execFileSync('python', ['scripts/scrape_package.py', '--quiet', url, tmpOut], { stdio: 'inherit' });
        } else {
          console.log('🔸 DRY RUN - scraper not executed');
        }

        const scrape = dryRun ? null : JSON.parse(fs.readFileSync(tmpOut, 'utf-8')) as any;
        if (!dryRun) {
          fs.unlinkSync(tmpOut);
        }

        const warnings: string[] = [];
        const offers: any[] = [];

        if (!dryRun) {
          const normalized = normalizeScrapeToOffer(scrape, pax, warnings);
          offers.push(normalized);
        }

        if (!dryRun) {
          sm.importPackageOffers(
            destination,
            offers[0]?.source_id || 'unknown',
            offers,
            `Imported from scrape-package CLI (${offers.length} offer)`,
            warnings
          );
          sm.save();
          console.log('✅ Imported offers into process_3_4_packages.results.offers');

          const best = offers[0]?.best_value?.date;
          if (best) {
            console.log(`\nNext action: npx ts-node src/cli/travel-update.ts select-offer ${offers[0].id} ${best}`);
          } else {
            console.log('\nNext action: review offers then run select-offer <offer-id> <date>');
          }
        }

        break;
      }

      case 'set-dates': {
        const [, startDate, endDate, ...reasonParts] = cleanArgs;
        if (!startDate || !endDate) {
          console.error('Error: set-dates requires <start> and <end> dates');
          console.error('Example: set-dates 2026-02-13 2026-02-17 "Agent offered Feb 13"');
          process.exit(1);
        }

        const reason = reasonParts.join(' ') || undefined;
        
        console.log(`\n📅 Setting dates: ${formatDate(startDate)} → ${formatDate(endDate)}`);
        if (reason) console.log(`   Reason: ${reason}`);

        if (!dryRun) {
          sm.setDateAnchor(startDate, endDate, reason);
          sm.save();
          console.log('✅ Dates updated and cascade triggered');
        } else {
          console.log('🔸 DRY RUN - no changes saved');
        }

        if (verbose) showStatus(sm);
        break;
      }

      case 'update-offer': {
        const [, offerId, date, availability, priceStr, seatsStr, source] = cleanArgs;
        if (!offerId || !date || !availability) {
          console.error('Error: update-offer requires <offer-id> <date> <availability>');
          console.error('Example: update-offer besttour_TYO05MM260211AM 2026-02-13 available 27888 2 agent');
          process.exit(1);
        }

        if (!['available', 'sold_out', 'limited'].includes(availability)) {
          console.error('Error: availability must be: available | sold_out | limited');
          process.exit(1);
        }

        const price = priceStr ? parseInt(priceStr, 10) : undefined;
        const seats = seatsStr ? parseInt(seatsStr, 10) : undefined;

        console.log(`\n📝 Updating offer availability:`);
        console.log(`   Offer: ${offerId}`);
        console.log(`   Date: ${formatDate(date)}`);
        console.log(`   Availability: ${availability}`);
        if (price) console.log(`   Price: TWD ${price.toLocaleString()}`);
        if (seats !== undefined) console.log(`   Seats: ${seats}`);
        if (source) console.log(`   Source: ${source}`);

        if (!dryRun) {
          sm.updateOfferAvailability(
            offerId,
            date,
            availability as 'available' | 'sold_out' | 'limited',
            price,
            seats,
            source || 'cli'
          );
          sm.save();
          console.log('✅ Offer availability updated');
        } else {
          console.log('🔸 DRY RUN - no changes saved');
        }
        break;
      }

      case 'select-offer': {
        const [, offerId, date] = cleanArgs;
        const noPopulate = args.includes('--no-populate');

        if (!offerId || !date) {
          console.error('Error: select-offer requires <offer-id> <date>');
          console.error('Example: select-offer besttour_TYO05MM260211AM 2026-02-13');
          process.exit(1);
        }

        console.log(`\n🎯 Selecting offer:`);
        console.log(`   Offer: ${offerId}`);
        console.log(`   Date: ${formatDate(date)}`);
        console.log(`   Populate P3/P4: ${!noPopulate}`);

        if (!dryRun) {
          sm.selectOffer(offerId, date, !noPopulate);
          sm.save();
          console.log('✅ Offer selected');
          if (!noPopulate) {
            console.log('✅ P3 (transportation) and P4 (accommodation) populated from package');
          }
        } else {
          console.log('🔸 DRY RUN - no changes saved');
        }

        if (verbose) showStatus(sm);
        break;
      }

      default:
        console.error(`Unknown command: ${command}`);
        console.log(HELP);
        process.exit(1);
    }
  } catch (error) {
    console.error(`\n❌ Error: ${(error as Error).message}`);
    if (verbose) {
      console.error((error as Error).stack);
    }
    process.exit(1);
  }
}

function normalizeScrapeToOffer(scrape: any, pax: number, warnings: string[]): any {
  const url: string = scrape?.url || '';
  const scrapedAt: string = scrape?.scraped_at || new Date().toISOString();
  const sourceId = url.includes('besttour.com.tw') ? 'besttour' : 'unknown';
  const productCode = url.split('/').filter(Boolean).pop() || 'unknown';
  const id = `${sourceId}_${productCode}`;

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
    currency: 'TWD',
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

function computeBestValue(datePricing: any, pax: number): { date: string; price_per_person: number; price_total: number; availability: any; seats_remaining?: number } | null {
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

main();
