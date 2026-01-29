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
import type { ProcessId, TransportOption } from '../state/types';
import {
  validateDestinationRef,
  validateDestinationRefConsistency,
  type DestinationRef,
} from '../state/destination-ref-schema';
import {
  resolveDestinationRefPath as configResolveDestinationRefPath,
  getAvailableDestinations,
  getOtaSourceCurrency,
} from '../config/loader';
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

  scaffold-itinerary [--dest slug] [--force]
    Create day skeletons for P5 itinerary based on date anchor.
    Generates arrival/full/departure day structures with flight transit notes.
    Use --force to overwrite existing itinerary.
    Example: scaffold-itinerary

  populate-itinerary --goals "<cluster1,cluster2,...>" [--pace relaxed|balanced|packed] [--assign "<cluster:day,...>"] [--dest slug] [--force]
    Populate itinerary sessions by adding activities from destination clusters (incremental; does not overwrite days).
    Example: populate-itinerary --goals "chanel_shopping,omiyage_premium,teamlab_roppongi,asakusa_classic" --pace balanced

  mark-booked [--dest slug]
    Mark package, flight, and hotel as booked (selected/populated → booking → booked).
    Use after user confirms booking is complete.
    Example: mark-booked

  set-airport-transfer <arrival|departure> <planned|booked> --selected "<title|route|duration_min?|price_yen?|schedule?>" [--candidate "<...>"]...
    Set airport transfer plan (selected + candidates) for arrival/departure.
    Spec fields are pipe-delimited. Only title and route are required.
    Example: set-airport-transfer arrival planned --selected "Limousine Bus|NRT T2 → Shiodome (Takeshiba)|85|3200|19:40 → ~21:05"

  set-activity-booking <day> <session> <activity> <status> [--ref <ref>] [--book-by <date>]
    Set booking status for an activity.
    day: Day number (1-indexed)
    session: morning | afternoon | evening
    activity: Activity ID or title (case-insensitive)
    status: not_required | pending | booked | waitlist
    Example: set-activity-booking 3 morning "teamLab Borderless" booked --ref "TLB-12345"
    Example: set-activity-booking 3 morning teamlab pending --book-by 2026-02-01

  set-activity-time <day> <session> <activity> [--start HH:MM] [--end HH:MM] [--fixed true|false]
    Set optional time fields for an activity (start/end/fixed).
    Example: set-activity-time 5 afternoon "Hotel checkout" --start 11:00 --fixed true

  set-session-time-range <day> <session> --start HH:MM --end HH:MM
    Set optional time boundaries for a session.
    Example: set-session-time-range 5 afternoon --start 11:00 --end 14:45

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
  --force      Allow overwrites / bypass safeguards (command-specific)
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

    const transfers = (p3?.airport_transfers as Record<string, unknown> | undefined) ?? undefined;
    if (transfers && (transfers['arrival'] || transfers['departure'])) {
      console.log('\nAirport Transfers:');
      console.log('─'.repeat(50));

      for (const dir of ['arrival', 'departure'] as const) {
        const seg = transfers[dir] as Record<string, unknown> | undefined;
        if (!seg) continue;

        const label = dir === 'arrival' ? 'Arrival' : 'Departure';
        const segStatus = (seg.status as string | undefined) ?? 'planned';
        console.log(`\n  ${label} (${segStatus})`);

        const selected = seg.selected as Record<string, unknown> | undefined | null;
        if (selected) {
          const title = selected.title as string | undefined;
          const route = selected.route as string | undefined;
          const duration = selected.duration_min as number | undefined;
          const price = selected.price_yen as number | undefined;
          const schedule = selected.schedule as string | undefined;
          console.log(`   ✓ ${title ?? ''}${price ? ` (¥${price.toLocaleString()})` : ''}${duration ? ` ~${duration} min` : ''}`);
          if (route) console.log(`     ${route}`);
          if (schedule) console.log(`     Schedule: ${schedule}`);
        }

        const candidates = seg.candidates as Array<Record<string, unknown>> | undefined;
        if (Array.isArray(candidates) && candidates.length > 0) {
          console.log('   Candidates:');
          for (const c of candidates.slice(0, 5)) {
            const title = c.title as string | undefined;
            const route = c.route as string | undefined;
            const duration = c.duration_min as number | undefined;
            const price = c.price_yen as number | undefined;
            console.log(`    - ${title ?? ''}${price ? ` (¥${price.toLocaleString()})` : ''}${duration ? ` ~${duration} min` : ''}${route ? ` — ${route}` : ''}`);
          }
          if (candidates.length > 5) console.log(`    ... and ${candidates.length - 5} more`);
        }
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

    // Show fixed-time activities (deadlines, reservations)
    const p5 = destObj.process_5_daily_itinerary as Record<string, unknown> | undefined;
    const days = p5?.days as Array<Record<string, unknown>> | undefined;
    if (Array.isArray(days) && days.length > 0) {
      const fixedActivities: Array<{
        day: number;
        date: string;
        session: string;
        title: string;
        start?: string;
        end?: string;
        bookingStatus?: string;
        bookingRef?: string;
      }> = [];

      for (const day of days) {
        const dayNum = day.day_number as number;
        const dayDate = day.date as string;
        for (const sessionName of ['morning', 'afternoon', 'evening'] as const) {
          const session = day[sessionName] as Record<string, unknown> | undefined;
          const activities = session?.activities as Array<unknown> | undefined;
          if (!Array.isArray(activities)) continue;

          for (const act of activities) {
            if (typeof act === 'string') continue;
            const a = act as Record<string, unknown>;
            if (a.is_fixed_time || a.booking_status === 'booked' || a.booking_required) {
              fixedActivities.push({
                day: dayNum,
                date: dayDate,
                session: sessionName,
                title: (a.title as string) ?? 'Untitled',
                start: a.start_time as string | undefined,
                end: a.end_time as string | undefined,
                bookingStatus: a.booking_status as string | undefined,
                bookingRef: a.booking_ref as string | undefined,
              });
            }
          }
        }
      }

      if (fixedActivities.length > 0) {
        console.log('\nFixed-Time Activities & Reservations:');
        console.log('─'.repeat(50));
        for (const fa of fixedActivities) {
          const timeStr = fa.start && fa.end ? `${fa.start}-${fa.end}`
            : fa.start ? `${fa.start}`
            : fa.end ? `by ${fa.end}`
            : '';
          const statusIcon = fa.bookingStatus === 'booked' ? '🎫'
            : fa.bookingStatus === 'pending' ? '⏳'
            : '📌';
          const refStr = fa.bookingRef ? ` [${fa.bookingRef}]` : '';
          console.log(`  ${statusIcon} Day ${fa.day} ${fa.session.padEnd(9)} ${timeStr.padEnd(11)} ${fa.title}${refStr}`);
        }
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

  const optionValues = (name: string): string[] => {
    const values: string[] = [];
    for (let i = 0; i < args.length; i++) {
      const a = args[i];
      if (a.startsWith(`${name}=`)) {
        values.push(a.slice(name.length + 1));
        continue;
      }
      if (a === name && i + 1 < args.length) {
        values.push(args[i + 1]);
        i++;
      }
    }
    return values;
  };

  const destOpt = optionValue('--dest');
  const paxOpt = optionValue('--pax');
  const planOpt = optionValue('--plan');
  const stateOpt = optionValue('--state');
  const goalsOpt = optionValue('--goals');
  const paceOpt = optionValue('--pace');
  const assignOpt = optionValue('--assign');
  const refOpt = optionValue('--ref');
  const bookByOpt = optionValue('--book-by');
  const selectedOpt = optionValue('--selected');
  const candidateOpts = optionValues('--candidate');
  const startOpt = optionValue('--start');
  const endOpt = optionValue('--end');
  const fixedOpt = optionValue('--fixed');

  // Filter out flags/options from args
  const optionsWithValues = new Set(['--dest', '--pax', '--plan', '--state', '--goals', '--pace', '--assign', '--ref', '--book-by', '--selected', '--candidate', '--start', '--end', '--fixed']);
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

      case 'scaffold-itinerary': {
        const destination = destOpt || sm.getActiveDestination();
        const force = args.includes('--force');
        const plan = sm.getPlan();
        const destObj = plan.destinations[destination] as Record<string, unknown> | undefined;

        if (!destObj) {
          console.error(`Error: Destination not found: ${destination}`);
          process.exit(1);
        }

        // Check for existing itinerary
        const existingP5 = destObj.process_5_daily_itinerary as Record<string, unknown> | undefined;
        const existingDays = existingP5?.days as unknown[];
        if (existingDays && existingDays.length > 0 && !force && !dryRun) {
          console.error('Error: Itinerary already has days. Use --force to overwrite.');
          process.exit(1);
        }
        if (existingDays && existingDays.length > 0 && !force && dryRun) {
          console.log('ℹ️  Itinerary already has days; DRY RUN will preview a fresh scaffold. Use --force to apply.');
        }

        // Get date anchor from destination or global
        const destAnchor = destObj.process_1_date_anchor as Record<string, unknown> | undefined;
        const confirmedDates = destAnchor?.confirmed_dates as { start: string; end: string } | undefined;

        if (!confirmedDates?.start || !confirmedDates?.end) {
          console.error('Error: No confirmed dates found in date anchor');
          console.error('Set dates first: set-dates <start> <end>');
          process.exit(1);
        }

        // Get flight times from P3 for transit notes
        const p3 = destObj.process_3_transportation as Record<string, unknown> | undefined;
        const flight = p3?.flight as Record<string, unknown> | undefined;
        const outbound = flight?.outbound as Record<string, unknown> | undefined;
        const returnFlight = flight?.return as Record<string, unknown> | undefined;

        const arrivalNote = outbound?.arrival_time
          ? `Arrive ${outbound.arrival_airport_code || 'airport'} ${outbound.arrival_time} → hotel`
          : null;
        const departureNote = returnFlight?.departure_time
          ? `Hotel → ${returnFlight.departure_airport_code || 'airport'} for ${returnFlight.departure_time} flight`
          : null;

        const startDate = new Date(confirmedDates.start);
        const endDate = new Date(confirmedDates.end);
        const days: Record<string, unknown>[] = [];

        // Generate day skeletons
        let dayNumber = 1;
        const current = new Date(startDate);
        while (current <= endDate) {
          const dateStr = current.toISOString().split('T')[0];
          const isFirst = dayNumber === 1;
          const isLast = current.getTime() === endDate.getTime();

          let dayType: string;
          if (isFirst) dayType = 'arrival';
          else if (isLast) dayType = 'departure';
          else dayType = 'full';

          // Add transit notes for arrival/departure days
          const morningTransit = isFirst ? arrivalNote : null;
          const eveningTransit = isLast ? departureNote : null;

          days.push({
            date: dateStr,
            day_number: dayNumber,
            day_type: dayType,
            status: 'draft',
            morning: { focus: null, activities: [], meals: [], transit_notes: morningTransit, booking_notes: null },
            afternoon: { focus: null, activities: [], meals: [], transit_notes: null, booking_notes: null },
            evening: { focus: null, activities: [], meals: [], transit_notes: eveningTransit, booking_notes: null },
          });

          dayNumber++;
          current.setDate(current.getDate() + 1);
        }

        console.log(`\n📅 Scaffolding itinerary for ${destination}:`);
        console.log(`   Dates: ${formatDate(confirmedDates.start)} → ${formatDate(confirmedDates.end)}`);
        console.log(`   Days: ${days.length}`);
        if (arrivalNote) console.log(`   Arrival: ${arrivalNote}`);
        if (departureNote) console.log(`   Departure: ${departureNote}`);
        console.log('');
        for (const day of days) {
          const icon = day.day_type === 'arrival' ? '✈️' : day.day_type === 'departure' ? '🛫' : '📍';
          console.log(`   ${icon} Day ${day.day_number}: ${day.date} (${day.day_type})`);
        }

        if (!dryRun) {
          sm.scaffoldItinerary(destination, days, force);
          sm.save();
          console.log('\n✅ Itinerary scaffolded');
          console.log('\nNext action: Review day structure, then populate activities with /p5-itinerary');
        } else {
          console.log('\n🔸 DRY RUN - no changes saved');
        }

        break;
      }

      case 'mark-booked': {
        const destination = destOpt || sm.getActiveDestination();
        const plan = sm.getPlan();
        const destObj = plan.destinations[destination] as Record<string, unknown> | undefined;

        if (!destObj) {
          console.error(`Error: Destination not found: ${destination}`);
          process.exit(1);
        }

        console.log(`\n🎫 Marking booking as confirmed for ${destination}:`);

        // Processes to mark as booked: p3_4_packages, p3_transportation, p4_accommodation
        const processesToBook: Array<{ id: ProcessId; name: string }> = [
          { id: 'process_3_4_packages', name: 'P3+4 Packages' },
          { id: 'process_3_transportation', name: 'P3 Transport' },
          { id: 'process_4_accommodation', name: 'P4 Accommodation' },
        ];

        for (const p of processesToBook) {
          const currentStatus = sm.getProcessStatus(destination, p.id);
          if (!currentStatus) {
            console.log(`   ⏭️  ${p.name}: skipped (no status)`);
            continue;
          }

          if (currentStatus === 'booked' || currentStatus === 'confirmed') {
            console.log(`   ✓  ${p.name}: already ${currentStatus}`);
            continue;
          }

          // Valid starting states: selected, populated
          if (!['selected', 'populated'].includes(currentStatus)) {
            console.log(`   ⚠️  ${p.name}: cannot book from ${currentStatus}`);
            continue;
          }

          if (!dryRun) {
            // Transition: selected/populated → booking → booked
            sm.setProcessStatus(destination, p.id, 'booking');
            sm.setProcessStatus(destination, p.id, 'booked');
            sm.clearDirty(destination, p.id);
          }
          console.log(`   ✅ ${p.name}: ${currentStatus} → booking → booked`);
        }

        if (!dryRun) {
          // Emit booking confirmation event
          sm.emitEvent({
            event: 'booking_confirmed',
            destination,
            data: {
              processes: processesToBook.map(p => p.id),
              confirmed_at: sm.now(),
            },
          });

          // Update next actions
          sm.setNextActions([
            'plan_daily_itinerary',
            'book_teamlab_tickets',
            'research_restaurant_reservations',
          ]);

          // Update focus to itinerary
          sm.setFocus(destination, 'process_5_daily_itinerary');

          sm.save();
          console.log('\n✅ Booking marked as confirmed');
          console.log('\nNext action: Plan daily itinerary with scaffold-itinerary or /p5-itinerary');
        } else {
          console.log('\n🔸 DRY RUN - no changes saved');
        }

        break;
      }

      case 'populate-itinerary': {
        const destination = destOpt || sm.getActiveDestination();
        const force = args.includes('--force');
        const pace = (paceOpt || 'balanced').toLowerCase();
        if (!['relaxed', 'balanced', 'packed'].includes(pace)) {
          console.error('Error: --pace must be one of: relaxed | balanced | packed');
          process.exit(1);
        }

        if (!goalsOpt) {
          console.error('Error: populate-itinerary requires --goals "<cluster1,cluster2,...>"');
          process.exit(1);
        }

        const goals = goalsOpt
          .split(',')
          .map(s => s.trim())
          .filter(Boolean);

        if (goals.length === 0) {
          console.error('Error: --goals had no usable cluster IDs');
          process.exit(1);
        }

        const plan = sm.getPlan();
        const destObj = plan.destinations[destination] as Record<string, unknown> | undefined;
        if (!destObj) {
          console.error(`Error: Destination not found: ${destination}`);
          process.exit(1);
        }

        const p5 = destObj.process_5_daily_itinerary as Record<string, unknown> | undefined;
        const days = (p5?.days as Array<Record<string, unknown>> | undefined) ?? [];
        if (days.length === 0) {
          console.error('Error: No itinerary days found. Run scaffold-itinerary first.');
          process.exit(1);
        }

        // Destination reference selection (currently Tokyo-only).
        const refPath = resolveDestinationRefPath(destination);
        if (!refPath) {
          const available = getAvailableDestinations();
          console.error(`Error: No destination reference available for ${destination}.`);
          console.error('Fix: add/update entry in data/destinations.json and ensure the ref_path exists.');
          console.error(`Available destinations: ${available.join(', ')}`);
          process.exit(1);
        }

        // Load and validate destination reference with Zod
        let ref: DestinationRef;
        try {
          const rawRef = JSON.parse(fs.readFileSync(refPath, 'utf-8'));
          ref = validateDestinationRef(rawRef, refPath);

          // Check internal consistency (dangling references)
          const refWarnings = validateDestinationRefConsistency(ref, refPath);
          if (refWarnings.length > 0 && verbose) {
            console.log('\n⚠️  Destination reference consistency warnings:');
            for (const w of refWarnings.slice(0, 5)) {
              console.log(`   - ${w}`);
            }
            if (refWarnings.length > 5) {
              console.log(`   ... and ${refWarnings.length - 5} more`);
            }
          }
        } catch (error) {
          console.error(`Error: Failed to load destination reference: ${refPath}`);
          console.error((error as Error).message);
          process.exit(1);
        }

        const poiById = new Map(ref.pois.map((p) => [p.id, p]));
        const areaNameById = new Map(ref.areas.map((a) => [a.id, a.name]));
        const clusters = ref.clusters;

        // Plan allocation: map clusters to day numbers.
        const explicitAssignments = parseAssignments(assignOpt);
        const allocation = allocateClustersToDays(goals, days, explicitAssignments);

        const plannedAdds: Array<{ day: number; session: 'morning' | 'afternoon' | 'evening'; poiId: string; title: string }> = [];
        const skipped: string[] = [];

        for (const item of allocation) {
          const cluster = clusters[item.clusterId];
          if (!cluster) {
            skipped.push(`Unknown cluster: ${item.clusterId}`);
            continue;
          }

          const poiIds = cluster.pois;
          if (poiIds.length === 0) {
            skipped.push(`Cluster has no POIs: ${item.clusterId}`);
            continue;
          }

          const day = days.find(d => d.day_number === item.dayNumber);
          if (!day) continue;

          const sessionOrder = getSessionOrderForDayType(day.day_type as string);
          const sessionCount = pace === 'relaxed' ? 1 : pace === 'balanced' ? 2 : 3;
          const usedSessions = sessionOrder.slice(0, Math.min(sessionCount, sessionOrder.length));
          const perSession = chunkEvenly(poiIds, usedSessions.length);

          // Theme/focus: set if empty (or if force).
          if (!dryRun) {
            if (force || !day.theme) {
              sm.setDayTheme(destination, item.dayNumber, cluster.name);
            }
            for (const sess of usedSessions) {
              const sessObj = day[sess] as Record<string, unknown> | undefined;
              const currentFocus = sessObj?.focus as string | null | undefined;
              if (force || !currentFocus) {
                sm.setSessionFocus(destination, item.dayNumber, sess, cluster.name);
              }
            }
          }

          for (let i = 0; i < usedSessions.length; i++) {
            const session = usedSessions[i];
            for (const poiId of perSession[i]) {
              const poi = poiById.get(poiId);
              if (!poi) {
                skipped.push(`Missing POI in ref: ${poiId}`);
                continue;
              }

              const title = poi.title;
              const existing = ((day[session] as Record<string, unknown>)?.activities as Array<{ title?: string }> | undefined) ?? [];
              const hasDup = existing.some(a => (a?.title || '').toLowerCase() === title.toLowerCase());
              if (hasDup && !force) {
                continue;
              }

              plannedAdds.push({ day: item.dayNumber, session, poiId, title });

              if (!dryRun) {
                const areaId = poi.area;
                const areaName = areaNameById.get(areaId) || areaId;
                const notesParts = [
                  poi.notes,
                  poi.hours ? `Hours: ${poi.hours}` : null,
                  poi.address ? `Address: ${poi.address}` : null,
                ].filter(Boolean) as string[];

                sm.addActivity(destination, item.dayNumber, session, {
                  title,
                  area: areaName,
                  nearest_station: poi.nearest_station ?? undefined,
                  duration_min: poi.duration_min ?? undefined,
                  booking_required: poi.booking_required ?? false,
                  booking_url: poi.booking_url ?? undefined,
                  cost_estimate: poi.cost_estimate ?? undefined,
                  tags: poi.tags ?? [],
                  notes: notesParts.length ? notesParts.join(' | ') : undefined,
                  priority: poi.booking_required ? 'must' : 'want',
                });
              }
            }
          }
        }

        console.log(`\n🧩 populate-itinerary (${destination})`);
        console.log(`   Pace: ${pace}`);
        console.log(`   Goals: ${goals.join(', ')}`);
        if (assignOpt) console.log(`   Assign: ${assignOpt}`);
        console.log(`   Ref: ${refPath}`);

        if (skipped.length > 0) {
          console.log('\nSkipped:');
          for (const s of skipped.slice(0, 10)) console.log(`  - ${s}`);
          if (skipped.length > 10) console.log(`  ... and ${skipped.length - 10} more`);
        }

        console.log(`\nPlanned additions: ${plannedAdds.length}`);
        for (const a of plannedAdds.slice(0, 20)) {
          console.log(`  - Day ${a.day} ${a.session}: ${a.title}`);
        }
        if (plannedAdds.length > 20) console.log(`  ... and ${plannedAdds.length - 20} more`);

        if (!dryRun) {
          sm.save();
          console.log('\n✅ Itinerary populated (incremental adds)');
          console.log('\nNext action: run status --full, then adjust with updateActivity/removeActivity as needed');
        } else {
          console.log('\n🔸 DRY RUN - no changes saved');
        }

        break;
      }

      case 'set-airport-transfer': {
        const [, direction, status] = cleanArgs;

        if (!direction || !status) {
          console.error('Error: set-airport-transfer requires <arrival|departure> <planned|booked>');
          console.error('Example: set-airport-transfer arrival planned --selected "Limousine Bus|NRT T2 → Shiodome|85|3200|19:40 → ~21:05"');
          process.exit(1);
        }

        if (!['arrival', 'departure'].includes(direction)) {
          console.error('Error: <arrival|departure> must be one of: arrival | departure');
          process.exit(1);
        }

        if (!['planned', 'booked'].includes(status)) {
          console.error('Error: <planned|booked> must be one of: planned | booked');
          process.exit(1);
        }

        if (!selectedOpt) {
          console.error('Error: set-airport-transfer requires --selected "<title|route|...>"');
          process.exit(1);
        }

        const destination = destOpt || sm.getActiveDestination();
        const selected = parseTransferSpec(direction as 'arrival' | 'departure', selectedOpt);
        const candidates = candidateOpts.map(c => parseTransferSpec(direction as 'arrival' | 'departure', c));

        console.log(`\n🚌 Setting airport transfer:`);
        console.log(`   Destination: ${destination}`);
        console.log(`   Direction: ${direction}`);
        console.log(`   Status: ${status}`);
        console.log(`   Selected: ${selected.title}`);
        if (candidates.length) console.log(`   Candidates: ${candidates.length}`);

        if (!dryRun) {
          sm.setAirportTransferSegment(destination, direction as 'arrival' | 'departure', {
            status: status as 'planned' | 'booked',
            selected,
            candidates,
          });
          sm.save();
          console.log('✅ Airport transfer updated');
        } else {
          console.log('🔸 DRY RUN - no changes saved');
        }

        if (verbose) showStatus(sm, { full });
        break;
      }

      case 'set-activity-time': {
        const [, dayStr, session, activity] = cleanArgs;
        if (!dayStr || !session || !activity) {
          console.error('Error: set-activity-time requires <day> <session> <activity>');
          console.error('Example: set-activity-time 5 afternoon "Hotel checkout" --start 11:00 --fixed true');
          process.exit(1);
        }

        const dayNumber = parseInt(dayStr, 10);
        if (!Number.isFinite(dayNumber) || dayNumber <= 0) {
          console.error('Error: <day> must be a positive integer (1-indexed day number)');
          process.exit(1);
        }

        const validSessions = ['morning', 'afternoon', 'evening'];
        if (!validSessions.includes(session)) {
          console.error('Error: <session> must be one of: morning | afternoon | evening');
          process.exit(1);
        }

        const parseFixed = (value: string | undefined): boolean | undefined => {
          if (value === undefined) return undefined;
          const v = value.toLowerCase();
          if (['true', '1', 'yes', 'y'].includes(v)) return true;
          if (['false', '0', 'no', 'n'].includes(v)) return false;
          throw new Error(`Invalid --fixed value: ${value} (use true|false)`);
        };

        let isFixed: boolean | undefined;
        try {
          isFixed = parseFixed(fixedOpt);
        } catch (e) {
          console.error(`Error: ${(e as Error).message}`);
          process.exit(1);
        }

        if (startOpt === undefined && endOpt === undefined && isFixed === undefined) {
          console.error('Error: set-activity-time requires at least one of: --start, --end, --fixed');
          process.exit(1);
        }

        const destination = destOpt || sm.getActiveDestination();

        console.log(`\n⏱️  Setting activity time:`);
        console.log(`   Destination: ${destination}`);
        console.log(`   Day ${dayNumber} ${session}: "${activity}"`);
        if (startOpt) console.log(`   Start: ${startOpt}`);
        if (endOpt) console.log(`   End: ${endOpt}`);
        if (isFixed !== undefined) console.log(`   Fixed: ${isFixed}`);

        if (!dryRun) {
          sm.setActivityTime(destination, dayNumber, session as any, activity, {
            start_time: startOpt,
            end_time: endOpt,
            is_fixed_time: isFixed,
          });
          sm.save();
          console.log('✅ Activity time updated');
        } else {
          console.log('🔸 DRY RUN - no changes saved');
        }

        if (verbose) showStatus(sm, { full });
        break;
      }

      case 'set-session-time-range': {
        const [, dayStr, session] = cleanArgs;
        if (!dayStr || !session) {
          console.error('Error: set-session-time-range requires <day> <session>');
          console.error('Example: set-session-time-range 5 afternoon --start 11:00 --end 14:45');
          process.exit(1);
        }

        const dayNumber = parseInt(dayStr, 10);
        if (!Number.isFinite(dayNumber) || dayNumber <= 0) {
          console.error('Error: <day> must be a positive integer (1-indexed day number)');
          process.exit(1);
        }

        const validSessions = ['morning', 'afternoon', 'evening'];
        if (!validSessions.includes(session)) {
          console.error('Error: <session> must be one of: morning | afternoon | evening');
          process.exit(1);
        }

        if (!startOpt || !endOpt) {
          console.error('Error: set-session-time-range requires --start HH:MM and --end HH:MM');
          process.exit(1);
        }

        const destination = destOpt || sm.getActiveDestination();
        console.log(`\n🕒 Setting session time range:`);
        console.log(`   Destination: ${destination}`);
        console.log(`   Day ${dayNumber} ${session}: ${startOpt} → ${endOpt}`);

        if (!dryRun) {
          sm.setSessionTimeRange(destination, dayNumber, session as any, startOpt, endOpt);
          sm.save();
          console.log('✅ Session time range updated');
        } else {
          console.log('🔸 DRY RUN - no changes saved');
        }

        if (verbose) showStatus(sm, { full });
        break;
      }

      case 'set-activity-booking': {
        const [, dayStr, session, activity, status] = cleanArgs;

        if (!dayStr || !session || !activity || !status) {
          console.error('Error: set-activity-booking requires <day> <session> <activity> <status>');
          console.error('Example: set-activity-booking 3 morning "teamLab Borderless" booked --ref "TLB-12345"');
          process.exit(1);
        }

        const dayNumber = parseInt(dayStr, 10);
        if (!Number.isFinite(dayNumber) || dayNumber <= 0) {
          console.error('Error: <day> must be a positive integer (1-indexed day number)');
          process.exit(1);
        }

        const validSessions = ['morning', 'afternoon', 'evening'];
        if (!validSessions.includes(session)) {
          console.error('Error: <session> must be one of: morning | afternoon | evening');
          process.exit(1);
        }

        const validStatuses = ['not_required', 'pending', 'booked', 'waitlist'];
        if (!validStatuses.includes(status)) {
          console.error('Error: <status> must be one of: not_required | pending | booked | waitlist');
          process.exit(1);
        }

        const destination = destOpt || sm.getActiveDestination();

        console.log(`\n🎫 Setting activity booking status:`);
        console.log(`   Destination: ${destination}`);
        console.log(`   Day ${dayNumber} ${session}: "${activity}"`);
        console.log(`   Status: ${status}`);
        if (refOpt) console.log(`   Reference: ${refOpt}`);
        if (bookByOpt) console.log(`   Book by: ${bookByOpt}`);

        if (!dryRun) {
          sm.setActivityBookingStatus(
            destination,
            dayNumber,
            session as 'morning' | 'afternoon' | 'evening',
            activity,
            status as 'not_required' | 'pending' | 'booked' | 'waitlist',
            refOpt,
            bookByOpt
          );
          sm.save();
          console.log('✅ Activity booking status updated');
        } else {
          console.log('🔸 DRY RUN - no changes saved');
        }

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
  const sourceId = inferSourceIdFromUrl(url);
  const productCode = url.split('/').filter(Boolean).pop() || 'unknown';
  const id = `${sourceId}_${productCode}`;

  // Get currency from OTA source config instead of hardcoding
  const currency = getOtaSourceCurrency(sourceId);

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

/**
 * Resolve destination reference file path using config loader.
 * Falls back to config-based resolution instead of hardcoded Tokyo check.
 */
function resolveDestinationRefPath(destinationSlug: string): string | null {
  return configResolveDestinationRefPath(destinationSlug);
}

/**
 * Infer OTA source ID from URL.
 */
function inferSourceIdFromUrl(url: string): string {
  if (url.includes('besttour.com.tw')) return 'besttour';
  if (url.includes('liontravel.com')) return 'liontravel';
  if (url.includes('tigerairtw.com')) return 'tigerair';
  if (url.includes('eztravel.com.tw')) return 'eztravel';
  if (url.includes('jalan.net')) return 'jalan';
  if (url.includes('travel.rakuten.co.jp')) return 'rakuten_travel';
  return 'unknown';
}

function allocateClustersToDays(
  clusterIds: string[],
  days: Array<Record<string, unknown>>,
  explicitAssignments?: Map<string, number>
): Array<{ clusterId: string; dayNumber: number }> {
  const fullDays = days.filter(d => d.day_type === 'full').map(d => d.day_number as number);
  const arrivalDay = days.find(d => d.day_type === 'arrival')?.day_number as number | undefined;
  const departureDay = days.find(d => d.day_type === 'departure')?.day_number as number | undefined;
  const dayNumbers = new Set(days.map(d => d.day_number as number));

  const assignedDays = new Set(explicitAssignments?.values() || []);
  const availableFullDays = fullDays.filter(d => !assignedDays.has(d));

  const result: Array<{ clusterId: string; dayNumber: number }> = [];
  let fullIdx = 0;

  for (const id of clusterIds) {
    let target: number | undefined;
    const explicit = explicitAssignments?.get(id);
    if (explicit && dayNumbers.has(explicit)) target = explicit;
    if (id.includes('last_day') && departureDay) target = departureDay;
    if (!target) {
      target = availableFullDays.length ? availableFullDays[fullIdx % availableFullDays.length] : undefined;
      fullIdx++;
    }
    if (!target && arrivalDay) target = arrivalDay;
    if (!target && departureDay) target = departureDay;
    if (!target) target = 1;

    result.push({ clusterId: id, dayNumber: target });
  }

  return result;
}

function parseAssignments(assignOpt: string | undefined): Map<string, number> | undefined {
  if (!assignOpt) return undefined;
  const map = new Map<string, number>();
  for (const part of assignOpt.split(',')) {
    const [clusterId, dayStr] = part.split(':').map(s => s.trim());
    if (!clusterId || !dayStr) continue;
    const day = parseInt(dayStr, 10);
    if (!Number.isFinite(day) || day <= 0) continue;
    map.set(clusterId, day);
  }
  return map.size ? map : undefined;
}

function getSessionOrderForDayType(dayType: string): Array<'morning' | 'afternoon' | 'evening'> {
  if (dayType === 'arrival') return ['afternoon', 'evening'];
  if (dayType === 'departure') return ['morning', 'afternoon'];
  return ['morning', 'afternoon', 'evening'];
}

function chunkEvenly<T>(items: T[], buckets: number): T[][] {
  const out: T[][] = Array.from({ length: buckets }, () => []);
  for (let i = 0; i < items.length; i++) {
    out[i % buckets].push(items[i]);
  }
  return out;
}

function slugify(text: string): string {
  return text
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '_')
    .replace(/^_+|_+$/g, '')
    .slice(0, 48);
}

function hashString(text: string): string {
  let hash = 5381;
  for (let i = 0; i < text.length; i++) {
    hash = ((hash << 5) + hash) + text.charCodeAt(i);
    hash |= 0;
  }
  return Math.abs(hash).toString(36);
}

function parseTransferSpec(direction: 'arrival' | 'departure', spec: string): TransportOption {
  const parts = spec.split('|').map(p => p.trim());
  const title = parts[0];
  const route = parts[1];
  if (!title || !route) {
    throw new Error(`Invalid transfer spec (need at least title|route): "${spec}"`);
  }

  const durationMin = parts[2] ? parseInt(parts[2], 10) : undefined;
  const priceYen = parts[3] ? parseInt(parts[3], 10) : undefined;
  const schedule = parts[4] || undefined;

  const id = `${direction}_${slugify(title)}_${hashString(route).slice(0, 6)}`;

  return {
    id,
    title,
    route,
    ...(Number.isFinite(durationMin) ? { duration_min: durationMin } : {}),
    ...(Number.isFinite(priceYen) ? { price_yen: priceYen } : {}),
    ...(schedule ? { schedule } : {}),
  };
}
