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
import type { ProcessId, TransportOption, FlightInfo, HotelInfo, AirportTransfers, SessionType } from '../state/types';
import {
  validateDestinationRef,
  validateDestinationRefConsistency,
  type DestinationRef,
} from '../state/destination-ref-schema';
import {
  resolveDestinationRefPath as configResolveDestinationRefPath,
  getAvailableDestinations,
  getOtaSourceCurrency,
  inferSourceIdFromUrl,
  inferRegionFromDestination,
} from '../config/loader';
import { PATHS, FRESHNESS } from '../config/constants';
import {
  validateIsoDate,
  validatePositiveInt,
  validateTime,
  validateDateRange,
} from '../types/validation';
import { defaultValidator } from '../validation/itinerary-validator';
import type { DaySummary, IssueSeverity, ResolvedActivity } from '../validation/types';
import { calculateLeave } from '../utils/holiday-calculator';
import { globalRegistry } from '../scrapers/registry';
import type { OtaSearchParams, ScrapeResult } from '../scrapers/types';
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
    Example: set-airport-transfer arrival planned --selected "Limousine Bus|NRT T1 → Shiodome (Takeshiba)|85|3200|19:40 → ~21:05"

  set-flight <outbound|return> [--dest slug] [--flight SL396] [--airline "Thai Lion Air"] [--airline-code SL]
    [--from TPE] [--dep-terminal T1] [--dep 09:00] [--to KIX] [--arr-terminal T1] [--arr 12:30]
    [--date 2026-02-24] [--booked-date 2026-02-24]
    Manually set/update a flight leg. Use when booking separately (bypasses cascade populate).
    Example: set-flight outbound --dest kyoto_2026 --flight SL396 --airline "Thai Lion Air" --from TPE --dep 09:00 --to KIX --arr 12:30

  set-hotel [--dest slug] [--name "Hotel Name"] [--check-in YYYY-MM-DD] [--access "route"] [--note "notes"]
    Manually set/update hotel info. Pipe-delimit multiple access directions.
    Example: set-hotel --dest kyoto_2026 --name "APA Hotel Kyoto Ekimae" --check-in 2026-02-24 --access "JR Kyoto Station 3min"

  set-activity-booking <day> <session> <activity> <status> [--ref <ref>] [--book-by <date>]
    Set booking status for an activity.
    day: Day number (1-indexed)
    session: morning | noon | afternoon | evening
    activity: Activity ID or title (case-insensitive)
    status: not_required | pending | booked | waitlist
    Example: set-activity-booking 3 morning "teamLab Borderless" booked --ref "TLB-12345"
    Example: set-activity-booking 3 morning teamlab pending --book-by 2026-02-01

  set-activity-time <day> <session> <activity> [--start HH:MM] [--end HH:MM] [--fixed true|false]
    Set optional time fields for an activity (start/end/fixed).
    Example: set-activity-time 5 afternoon "Hotel checkout" --start 11:00 --fixed true

  set-route-segment <day> <sort_order> <from> <to> <mode> [--duration <min>] [--notes "<text>"] [--start-time HH:MM] [--plan-id <id>]
    Upsert a route segment on a day (0-based sort_order). mode: transit | walking | driving
    Example: set-route-segment 1 0 "關渡" "嘟嘟房桃園機場貨運1站" driving --duration 45 --start-time 06:00
    Example: set-route-segment 1 1 "嘟嘟房桃園機場貨運1站" "桃園國際機場第一航廈" transit --duration 15 --start-time 06:45

  set-route-segments-bulk <day> --json '[{"from":"A","to":"B","mode":"walking","duration":5},...]' [--dest slug] [--plan-id <id>]
    Replace ALL route segments for a day with the provided JSON array. Auto-assigns sort_order.
    Example: set-route-segments-bulk 4 --json '[{"from":"hotel","to":"京都駅","mode":"walking","duration":3},{"from":"京都駅","to":"稲荷駅","mode":"transit","duration":5,"notes":"JR奈良線"}]'

  set-day-theme <day> [theme] [--zh "<zh_title>"] [--dest slug] [--plan-id <id>]
    Set day theme/title. Use --zh to set Traditional Chinese title (shown by default).
    Example: set-day-theme 1 --zh "抵達京都・安頓"
    Example: set-day-theme 2 "Kinkaku-ji day" --zh "金閣寺・伏見稲荷"

  set-activity-title <day> <session> <activity> "<new_title>" [--plan-id <id>]
    Rename an activity by title substring or ID.
    Example: set-activity-title 2 morning "Fushimi Inari" "金閣寺 (Kinkaku-ji)"

  delete-activity <day> <session> <activity_id_or_title> [--plan-id <id>]
    Remove an activity from a session. activity: ID or title substring.
    Aliases: remove-activity
    Example: delete-activity 2 morning "teamLab Borderless"

  set-tod-focus <day> <session> "<focus_text>" [--plan-id <id>]
    Set EN session focus summary.
    Aliases: set-session-focus
    Example: set-tod-focus 2 morning "Kitano Tenmangu → Kinkaku-ji"

  set-tod-time-range <day> <session> --start HH:MM --end HH:MM
    Set optional time boundaries for a session.
    Aliases: set-session-time-range
    Example: set-tod-time-range 5 afternoon --start 11:00 --end 14:45

  swap-days <dayA> <dayB> [--dest slug]
    Swap all activities between two days (preserves sessions).
    Useful for reordering itinerary without manual re-assignment.
    Example: swap-days 2 3

  validate-itinerary [--dest slug] [--severity error|warning|info] [--json]
    Validate itinerary for time conflicts, business hours, booking deadlines, and area efficiency.
    Example: validate-itinerary --severity warning

  fetch-weather [--dest slug] [--all]
    Fetch weather forecast from Open-Meteo and store on itinerary days.
    Requires itinerary to be scaffolded first. Dates must be within 16-day forecast window.
    Use --all to fetch weather for every destination that has itinerary days.
    Example: fetch-weather
    Example: fetch-weather --all

  search-offers --dest slug [--start YYYY-MM-DD] [--end YYYY-MM-DD] [--pax N] [--types package,flight,hotel] [--source id] [--json]
    Search across registered OTA scrapers (if any are registered at runtime).
    If --start/--end are omitted, uses the destination confirmed dates.
    Example: search-offers --dest tokyo_2026 --pax 2 --types package --json

  compare-offers --region <name> [--date YYYY-MM-DD] [--pax N] [--json]
    Compare scraped offers from scrapes/*.json files by region.
    Reads existing scraped data files (no new scraping).
    region: osaka, kansai, tokyo, etc. (matches filenames like *-osaka-*.json)
    Example: compare-offers --region osaka --date 2026-02-26 --pax 2

  view-prices --flights <file> [--hotel-per-night TWD] [--nights N] [--package TWD] [--pax N] [--json]
    Compare package vs separate booking (flight+hotel) across departure dates.
    Reads date-range flight data from scrape_date_range.py output.
    --flights: Path to date-range JSON file (required)
    --hotel-per-night: Hotel cost per night in TWD (default: auto-detect from scrapes/booking-*.json)
    --nights: Number of hotel nights (default: duration - 1 from flight data)
    --package: Package price for all pax in TWD (for comparison column)
    Example: view-prices --flights scrapes/date-range-prices.json --hotel-per-night 3000 --nights 4 --package 40740

  query-offers [--region name] [--start YYYY-MM-DD] [--end YYYY-MM-DD] [--sources csv] [--max-price N] [--fresh-hours N] [--max N] [--json]
    Query offers from Turso cloud database with filters.
    Example: query-offers --region kansai --start 2026-02-24 --end 2026-02-28

  check-freshness --source <id> [--region name] [--start YYYY-MM-DD] [--end YYYY-MM-DD] [--max-age N]
    Check if Turso has fresh data for a source/region. Returns skip/rescrape/no_data.
    Example: check-freshness --source besttour --region kansai

  sync-bookings [--plan path] [--state path] [--trip-id id] [--dry-run]
    Extract bookings from travel-plan.json and sync to Turso. Idempotent.
    Example: sync-bookings

  query-bookings [--dest slug] [--category package|transfer|activity] [--status pending|booked] [--trip-id id] [--json]
    Query bookings from Turso DB.
    Example: query-bookings --dest tokyo_2026 --status pending

  check-booking-integrity [--trip-id id]
    Compare bookings in plan JSON vs Turso DB.
    Example: check-booking-integrity

  run-status [run-id]
    Show details for a specific operation run, or the most recent run if no ID given.
    Example: run-status
    Example: run-status a1b2c3d4-...

  run-list [--status completed|failed|started] [--limit N]
    List recent operation runs for this plan.
    Example: run-list
    Example: run-list --status failed --limit 5

  status
    Show current plan status summary.

  itinerary [--dest slug]
    Show daily itinerary with transport details.

  transport [--dest slug]
    Show transport summary (airport + daily transit).

  bookings [--dest slug]
    Show pending bookings only.

  help
    Show this help message.

Options:
  --plan-id <id> Plan ID (preferred, e.g., 'kyoto-2026' or $TRAVEL_PLAN_ID)
  --plan <path>  Travel plan path (deprecated, use --plan-id instead)
  --state <path> State log path (default: ${PATHS.defaultState} or $TRAVEL_STATE_PATH)
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

  if (full) {
    const flight = sm.getFlightInfo();
    const hotel = sm.getHotelInfo();
    const transfers = sm.getAirportTransfers();

    if (flight?.outbound && (flight.outbound.flight_number || flight.outbound.departure_airport_code)) {
      console.log('\nFlight Details:');
      console.log('─'.repeat(50));
      const { airline, airline_code } = flight;
      const outbound = flight.outbound;
      console.log(`  ${[airline_code, outbound.flight_number].filter(Boolean).join(' ')}${airline ? ` (${airline})` : ''}`);
      const fmtAp = (leg: { departure_airport_code: string | null; departure_terminal: string | null; arrival_airport_code: string | null; arrival_terminal: string | null }, side: 'dep' | 'arr') => {
        const code = side === 'dep' ? (leg.departure_airport_code ?? '') : (leg.arrival_airport_code ?? '');
        const term = side === 'dep' ? (leg.departure_terminal ?? '') : (leg.arrival_terminal ?? '');
        return term ? `${code} ${term}` : code;
      };
      console.log(`  ${fmtAp(outbound, 'dep')} ${outbound.departure_time ?? ''} → ${fmtAp(outbound, 'arr')} ${outbound.arrival_time ?? ''}`);
      const inbound = flight.return;
      if (inbound && (inbound.flight_number || inbound.departure_airport_code)) {
        console.log(`  Return: ${inbound.flight_number ?? ''}`);
        console.log(`  ${fmtAp(inbound, 'dep')} ${inbound.departure_time ?? ''} → ${fmtAp(inbound, 'arr')} ${inbound.arrival_time ?? ''}`);
      }
    }

    if (transfers && (transfers.arrival || transfers.departure)) {
      console.log('\nAirport Transfers:');
      console.log('─'.repeat(50));

      for (const dir of ['arrival', 'departure'] as const) {
        const seg = transfers[dir];
        if (!seg) continue;

        const label = dir === 'arrival' ? 'Arrival' : 'Departure';
        const segStatus = seg.status ?? 'planned';
        console.log(`\n  ${label} (${segStatus})`);

        const selected = seg.selected;
        if (selected) {
          console.log(`   ✓ ${selected.title ?? ''}${selected.price_yen ? ` (¥${selected.price_yen.toLocaleString()})` : ''}${selected.duration_min ? ` ~${selected.duration_min} min` : ''}`);
          if (selected.route) console.log(`     ${selected.route}`);
          if (selected.schedule) console.log(`     Schedule: ${selected.schedule}`);
        }

        const candidates = seg.candidates;
        if (Array.isArray(candidates) && candidates.length > 0) {
          console.log('   Candidates:');
          for (const c of candidates.slice(0, 5)) {
            console.log(`    - ${c.title ?? ''}${c.price_yen ? ` (¥${c.price_yen.toLocaleString()})` : ''}${c.duration_min ? ` ~${c.duration_min} min` : ''}${c.route ? ` — ${c.route}` : ''}`);
          }
          if (candidates.length > 5) console.log(`    ... and ${candidates.length - 5} more`);
        }
      }
    }

    if (hotel && hotel.name) {
      console.log('\nHotel Details:');
      console.log('─'.repeat(50));
      console.log(`  ${hotel.name}`);
      if (hotel.access.length > 0) {
        console.log(`  Access: ${hotel.access.slice(0, 4).join(', ')}`);
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
        sessionStart?: string;
        sessionEnd?: string;
        bookingStatus?: string;
        bookingRef?: string;
        bookingRequired?: boolean;
        isFixedTime?: boolean;
      }> = [];

      for (const day of days) {
        const dayNum = day.day_number as number;
        const dayDate = day.date as string;
        for (const sessionName of ['morning', 'noon', 'afternoon', 'evening'] as const) {
          const session = day[sessionName] as Record<string, unknown> | undefined;
          const timeRange = session?.time_range as { start?: string; end?: string } | undefined;
          const activities = session?.activities as Array<unknown> | undefined;
          if (!Array.isArray(activities)) continue;

          for (const act of activities) {
            if (typeof act === 'string') continue;
            const a = act as Record<string, unknown>;
            const isFixedTime = Boolean(a.is_fixed_time);
            const bookingRequired = Boolean(a.booking_required);
            const bookingStatus = a.booking_status as string | undefined;
            const isReservation = bookingRequired || bookingStatus === 'booked' || bookingStatus === 'pending' || bookingStatus === 'waitlist';

            // Include:
            // - any fixed-time constraints (even if booking_required=false), and
            // - any reservation/ticket items (booking_required/booking_status)
            if (isFixedTime || isReservation) {
              fixedActivities.push({
                day: dayNum,
                date: dayDate,
                session: sessionName,
                title: (a.title as string) ?? 'Untitled',
                start: a.start_time as string | undefined,
                end: a.end_time as string | undefined,
                sessionStart: timeRange?.start,
                sessionEnd: timeRange?.end,
                bookingStatus,
                bookingRef: a.booking_ref as string | undefined,
                bookingRequired,
                isFixedTime,
              });
            }
          }
        }
      }

      if (fixedActivities.length > 0) {
        console.log('\nFixed-Time Activities & Reservations:');
        console.log('─'.repeat(50));

        const sessionOrder = { morning: 0, noon: 1, afternoon: 2, evening: 3 } as const;
        fixedActivities.sort((a, b) => (
          (a.day - b.day) ||
          ((sessionOrder as any)[a.session] - (sessionOrder as any)[b.session]) ||
          a.title.localeCompare(b.title)
        ));

        for (const fa of fixedActivities) {
          const timeStr = fa.start && fa.end ? `${fa.start}-${fa.end}`
            : fa.start ? `${fa.start}`
            : fa.end ? `by ${fa.end}`
            : fa.sessionStart && fa.sessionEnd ? `${fa.sessionStart}-${fa.sessionEnd}`
            : '';

          const statusIcon = fa.bookingStatus === 'booked' ? '🎫'
            : (fa.bookingRequired || fa.bookingStatus === 'pending' || fa.bookingStatus === 'waitlist') ? '⏳'
            : fa.isFixedTime ? '📌'
            : '📌';

          const refStr = fa.bookingRef ? ` [${fa.bookingRef}]` : '';
          console.log(`  ${statusIcon} Day ${fa.day} ${fa.session.padEnd(9)} ${timeStr.padEnd(11)} ${fa.title}${refStr}`);
        }
      }
    }
  }

  console.log('\n');
}

function showItinerary(sm: StateManager, destOpt?: string): void {
  const destination = sm.resolveDestination(destOpt);
  const plan = sm.getPlan();
  const destObj = plan.destinations[destination] as Record<string, unknown> | undefined;

  if (!destObj) {
    console.error(`Destination not found: ${destination}`);
    process.exit(1);
  }

  const p5 = destObj.process_5_daily_itinerary as Record<string, unknown> | undefined;
  const days = p5?.days as Array<Record<string, unknown>> | undefined;

  if (!days || days.length === 0) {
    console.log('No itinerary days found. Run scaffold-itinerary first.');
    return;
  }

  // Header
  const p1 = destObj.process_1_date_anchor as Record<string, unknown> | undefined;
  const confirmed = p1?.confirmed_dates as { start?: string; end?: string } | undefined;
  const startDate = confirmed?.start || '';
  const endDate = confirmed?.end || '';

  console.log('\n╔════════════════════════════════════════════════════════════╗');
  console.log('║                    ITINERARY                               ║');
  console.log('╚════════════════════════════════════════════════════════════╝\n');

  if (startDate && endDate) {
    console.log(`📅 ${formatDate(startDate)} → ${formatDate(endDate)} (${days.length} days)`);
  }

  // Flight info
  const p3 = destObj.process_3_transportation as Record<string, unknown> | undefined;
  const flight = p3?.flight as Record<string, unknown> | undefined;
  if (flight) {
    const airline = flight.airline as string | undefined;
    const airlineCode = flight.airline_code as string | undefined;
    const outbound = flight.outbound as Record<string, unknown> | undefined;
    const inbound = flight.return as Record<string, unknown> | undefined;
    if (outbound) {
      const fmtAp = (leg: Record<string, unknown>, side: 'dep' | 'arr') => {
        const code = side === 'dep' ? (leg.departure_airport_code as string ?? '') : (leg.arrival_airport_code as string ?? '');
        const term = side === 'dep' ? (leg.departure_terminal as string ?? '') : (leg.arrival_terminal as string ?? '');
        return term ? `${code} ${term}` : code;
      };
      const label = [airlineCode, outbound.flight_number].filter(Boolean).join(' ');
      const dep = `${fmtAp(outbound, 'dep')} ${outbound.departure_time ?? ''}`;
      const arr = `${fmtAp(outbound, 'arr')} ${outbound.arrival_time ?? ''}`;
      let line = `✈️  ${label}${airline ? ` (${airline})` : ''}: ${dep} → ${arr}`;
      if (inbound) {
        const rLabel = inbound.flight_number as string || '';
        line += ` / ${rLabel} ${fmtAp(inbound, 'dep')} ${inbound.departure_time ?? ''} → ${fmtAp(inbound, 'arr')} ${inbound.arrival_time ?? ''}`;
      }
      console.log(line);
    }
  }

  // Hotel info
  const p4 = destObj.process_4_accommodation as Record<string, unknown> | undefined;
  const hotel = p4?.hotel as Record<string, unknown> | undefined;
  if (hotel?.name) {
    const area = hotel.area as string | undefined;
    const access = hotel.access as string[] | undefined;
    let line = `🏨 ${hotel.name}`;
    if (area) line += ` (${area})`;
    console.log(line);
    if (Array.isArray(access) && access.length > 0) {
      console.log(`🚉 ${access.join(', ')}`);
    }
  }

  // Transit summary (fallback if no hotel access)
  if (!hotel?.name) {
    const transitSummary = p5?.transit_summary as Record<string, unknown> | undefined;
    if (transitSummary?.hotel_station) {
      console.log(`🚉 ${transitSummary.hotel_station}`);
    }
  }

  console.log('');

  // Each day
  for (const day of days) {
    const dayNum = day.day_number as number;
    const date = day.date as string;
    const theme = day.theme as string | undefined;
    const dayType = day.day_type as string | undefined;

    const dayLabel = dayType === 'arrival' ? '✈️ ARRIVAL' :
                     dayType === 'departure' ? '✈️ DEPARTURE' : '';

    console.log('─'.repeat(60));
    console.log(`Day ${dayNum} (${formatDate(date)}) ${dayLabel}`);
    if (theme) console.log(`Theme: ${theme}`);

    const weather = day.weather as { temp_high_c?: number; temp_low_c?: number; precipitation_pct?: number; weather_label?: string; weather_code?: number; source_id?: string; sourced_at?: string } | undefined;
    if (weather && weather.weather_label) {
      const icon = (weather.weather_code ?? 0) >= 71 && (weather.weather_code ?? 0) <= 77 ? '\u2744\uFE0F' :
                   (weather.precipitation_pct ?? 0) > 50 ? '\uD83C\uDF27\uFE0F' :
                   (weather.weather_code ?? 0) >= 2 ? '\u26C5' : '\u2600\uFE0F';
      const srcDate = weather.sourced_at ? weather.sourced_at.split('T')[0] : '';
      console.log(`${icon} ${weather.weather_label} | ${weather.temp_low_c}\u2013${weather.temp_high_c}\u00B0C | Rain: ${weather.precipitation_pct}%  (${weather.source_id || 'unknown'}, ${srcDate})`);
    }

    console.log('');

    for (const sessionName of ['morning', 'noon', 'afternoon', 'evening'] as const) {
      const session = day[sessionName] as Record<string, unknown> | undefined;
      if (!session) continue;

      const focus = session.focus as string | undefined;
      const activities = session.activities as Array<unknown> | undefined;
      const transitNotes = session.transit_notes as string | undefined;
      const meals = session.meals as string[] | undefined;

      if (!activities || activities.length === 0) continue;

      const sessionLabel = sessionName.charAt(0).toUpperCase() + sessionName.slice(1);
      console.log(`  【${sessionLabel}】${focus ? ` ${focus}` : ''}`);

      for (const act of activities) {
        if (typeof act === 'string') {
          console.log(`    • ${act}`);
        } else {
          const a = act as Record<string, unknown>;
          const title = a.title as string || '';
          const status = a.booking_status as string | undefined;
          const ref = a.booking_ref as string | undefined;
          const bookBy = a.book_by as string | undefined;

          const icon = status === 'booked' ? '🎫' :
                       status === 'pending' ? '⏳' :
                       a.booking_required ? '📋' : '•';

          let suffix = '';
          if (ref) suffix += ` [${ref}]`;
          if (status === 'pending' && bookBy) suffix += ` (book by ${bookBy})`;

          console.log(`    ${icon} ${title}${suffix}`);
        }
      }

      if (transitNotes) {
        console.log(`    🚃 ${transitNotes}`);
      }

      if (meals && meals.length > 0) {
        console.log(`    🍽️  ${meals.join(', ')}`);
      }

      console.log('');
    }

    // Route segments
    const routeSegments = day.route_segments as Array<Record<string, unknown>> | undefined;
    if (routeSegments && routeSegments.length > 0) {
      console.log('  🗺️  ROUTE:');
      for (const seg of routeSegments) {
        const from = seg.from_place as string;
        const to = seg.to_place as string;
        const mode = seg.mode as string;
        const dur = seg.duration_min != null ? ` (~${seg.duration_min} min)` : '';
        const notes = seg.notes as string | undefined;
        const modeIcon = mode === 'driving' ? '🚗' : mode === 'walking' ? '🚶' : '🚌';
        console.log(`    ${modeIcon} ${from} → ${to}${dur}${notes ? `  (${notes})` : ''}`);
      }
      console.log('');
    }
  }

  // Pending bookings summary
  const pendingBookings: Array<{ day: number; title: string; bookBy?: string }> = [];
  for (const day of days) {
    const dayNum = day.day_number as number;
    for (const sessionName of ['morning', 'noon', 'afternoon', 'evening'] as const) {
      const session = day[sessionName] as Record<string, unknown> | undefined;
      const activities = session?.activities as Array<unknown> | undefined;
      if (!activities) continue;

      for (const act of activities) {
        if (typeof act !== 'string') {
          const a = act as Record<string, unknown>;
          if (a.booking_status === 'pending' || (a.booking_required && !a.booking_status)) {
            pendingBookings.push({
              day: dayNum,
              title: a.title as string,
              bookBy: a.book_by as string | undefined,
            });
          }
        }
      }
    }
  }

  if (pendingBookings.length > 0) {
    console.log('─'.repeat(60));
    console.log('⏳ PENDING BOOKINGS');
    for (const pb of pendingBookings) {
      const deadline = pb.bookBy ? ` (by ${pb.bookBy})` : '';
      console.log(`  Day ${pb.day}: ${pb.title}${deadline}`);
    }
    console.log('');
  }
}

function showTransport(sm: StateManager, destOpt?: string): void {
  const destination = sm.resolveDestination(destOpt);
  const plan = sm.getPlan();
  const destObj = plan.destinations[destination] as Record<string, unknown> | undefined;

  if (!destObj) {
    console.error(`Destination not found: ${destination}`);
    process.exit(1);
  }

  console.log('\n╔════════════════════════════════════════════════════════════╗');
  console.log('║                   TRANSPORT                                ║');
  console.log('╚════════════════════════════════════════════════════════════╝\n');

  // Airport transfers (typed API)
  const transfers = sm.getAirportTransfers(destination);

  if (transfers) {
    console.log('✈️  AIRPORT TRANSFERS');
    console.log('─'.repeat(50));

    for (const dir of ['arrival', 'departure'] as const) {
      const seg = transfers[dir];
      if (!seg) continue;

      const status = seg.status || 'planned';
      const selected = seg.selected;
      const icon = status === 'booked' ? '🎫' : '📋';

      console.log(`\n${dir.toUpperCase()} (${status})`);
      if (selected) {
        console.log(`  ${icon} ${selected.title || ''}`);
        console.log(`     Route: ${selected.route || ''}`);
        if (selected.duration_min) console.log(`     Time: ~${selected.duration_min} min`);
        if (selected.price_yen) console.log(`     Price: ¥${selected.price_yen.toLocaleString()}`);
        if (selected.schedule) console.log(`     Schedule: ${selected.schedule}`);
      }
    }
    console.log('');
  }

  // Daily transit from itinerary
  const p5 = destObj.process_5_daily_itinerary as Record<string, unknown> | undefined;
  const transitSummary = p5?.transit_summary as Record<string, unknown> | undefined;

  if (transitSummary) {
    console.log('🚃 DAILY TRANSIT');
    console.log('─'.repeat(50));

    if (transitSummary.hotel_station) {
      console.log(`\nHome station: ${transitSummary.hotel_station}`);
    }
    if (transitSummary.ic_card) {
      console.log(`IC Card: ${transitSummary.ic_card}`);
    }
    if (transitSummary.daily_transit_cost) {
      console.log(`Daily cost: ${transitSummary.daily_transit_cost}`);
    }

    const keyLines = transitSummary.key_lines as string[] | undefined;
    if (keyLines && keyLines.length > 0) {
      console.log('\nKey lines:');
      for (const line of keyLines) {
        console.log(`  • ${line}`);
      }
    }

    const tips = transitSummary.tips as string[] | undefined;
    if (tips && tips.length > 0) {
      console.log('\nTips:');
      for (const tip of tips) {
        console.log(`  💡 ${tip}`);
      }
    }
  }

  // Per-day transit notes
  const days = p5?.days as Array<Record<string, unknown>> | undefined;
  if (days && days.length > 0) {
    console.log('\n\n📅 BY DAY');
    console.log('─'.repeat(50));

    for (const day of days) {
      const dayNum = day.day_number as number;
      const date = day.date as string;
      const theme = day.theme as string | undefined;

      console.log(`\nDay ${dayNum} (${formatDate(date)})${theme ? ` - ${theme}` : ''}`);

      for (const sessionName of ['morning', 'noon', 'afternoon', 'evening'] as const) {
        const session = day[sessionName] as Record<string, unknown> | undefined;
        const transitNotes = session?.transit_notes as string | undefined;
        if (transitNotes) {
          console.log(`  ${sessionName}: ${transitNotes}`);
        }
      }
    }
  }

  console.log('\n');
}

function showBookings(sm: StateManager, destOpt?: string): void {
  const destination = sm.resolveDestination(destOpt);
  const plan = sm.getPlan();
  const destObj = plan.destinations[destination] as Record<string, unknown> | undefined;

  if (!destObj) {
    console.error(`Destination not found: ${destination}`);
    process.exit(1);
  }

  console.log('\n╔════════════════════════════════════════════════════════════╗');
  console.log('║                   BOOKINGS                                 ║');
  console.log('╚════════════════════════════════════════════════════════════╝\n');

  // Package/flight/hotel status
  const p34 = destObj.process_3_4_packages as Record<string, unknown> | undefined;
  const packageStatus = p34?.status as string || 'pending';
  console.log('📦 PACKAGE');
  console.log('─'.repeat(50));
  console.log(`  Status: ${packageStatus === 'booked' ? '🎫 Booked' : '⏳ ' + packageStatus}`);

  if (packageStatus === 'booked' || packageStatus === 'selected') {
    const offerId = p34?.selected_offer_id as string | undefined;
    if (offerId) console.log(`  Offer: ${offerId}`);
  }
  console.log('');

  // Airport transfers
  const p3 = destObj.process_3_transportation as Record<string, unknown> | undefined;
  const transfers = p3?.airport_transfers as Record<string, unknown> | undefined;

  console.log('✈️  AIRPORT TRANSFERS');
  console.log('─'.repeat(50));
  for (const dir of ['arrival', 'departure'] as const) {
    const t = transfers?.[dir] as Record<string, unknown> | undefined;
    const status = t?.status as string || 'not set';
    const selected = t?.selected as Record<string, unknown> | undefined;
    const title = selected?.title as string || '(none)';
    const icon = status === 'booked' ? '🎫' : status === 'planned' ? '📋' : '❓';
    console.log(`  ${icon} ${dir}: ${title} (${status})`);
  }
  console.log('');

  // Activity bookings
  const p5 = destObj.process_5_daily_itinerary as Record<string, unknown> | undefined;
  const days = p5?.days as Array<Record<string, unknown>> | undefined;

  const pending: Array<{ day: number; title: string; bookBy?: string }> = [];
  const booked: Array<{ day: number; title: string; ref?: string }> = [];

  if (days) {
    for (const day of days) {
      const dayNum = day.day_number as number;
      for (const sessionName of ['morning', 'noon', 'afternoon', 'evening'] as const) {
        const session = day[sessionName] as Record<string, unknown> | undefined;
        const activities = session?.activities as Array<unknown> | undefined;
        if (!activities) continue;

        for (const act of activities) {
          if (typeof act !== 'string') {
            const a = act as Record<string, unknown>;
            const status = a.booking_status as string | undefined;
            const required = a.booking_required as boolean | undefined;
            const title = a.title as string;

            if (status === 'booked') {
              booked.push({ day: dayNum, title, ref: a.booking_ref as string | undefined });
            } else if (status === 'pending' || (required && !status)) {
              pending.push({ day: dayNum, title, bookBy: a.book_by as string | undefined });
            }
          }
        }
      }
    }
  }

  console.log('🎫 CONFIRMED');
  console.log('─'.repeat(50));
  if (booked.length === 0) {
    console.log('  (none)');
  } else {
    for (const b of booked) {
      const refStr = b.ref ? ` [${b.ref}]` : '';
      console.log(`  ✅ Day ${b.day}: ${b.title}${refStr}`);
    }
  }
  console.log('');

  console.log('⏳ PENDING');
  console.log('─'.repeat(50));
  if (pending.length === 0) {
    console.log('  (none)');
  } else {
    for (const p of pending) {
      const deadline = p.bookBy ? ` (by ${p.bookBy})` : '';
      console.log(`  ⏳ Day ${p.day}: ${p.title}${deadline}`);
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
  const planIdOpt = optionValue('--plan-id') || process.env.TRAVEL_PLAN_ID;
  const planOpt = optionValue('--plan');
  const stateOpt = optionValue('--state');
  
  // Deprecation warning for --plan (prefer --plan-id)
  if (planOpt && !planIdOpt) {
    console.warn('⚠️  --plan is deprecated. Use --plan-id or set TRAVEL_PLAN_ID env var instead.');
  }
  const goalsOpt = optionValue('--goals');
  const paceOpt = optionValue('--pace');
  const assignOpt = optionValue('--assign');
  const refOpt = optionValue('--ref');
  const bookByOpt = optionValue('--book-by');
  const durationOpt = optionValue('--duration');
  const notesOpt = optionValue('--notes');
  const selectedOpt = optionValue('--selected');
  const candidateOpts = optionValues('--candidate');
  const startOpt = optionValue('--start');
  const endOpt = optionValue('--end');
  const fixedOpt = optionValue('--fixed');
  const severityOpt = optionValue('--severity');
  const typesOpt = optionValue('--types');
  const sourceOpt = optionValue('--source');
  const flightsOpt = optionValue('--flights');
  const hotelPerNightOpt = optionValue('--hotel-per-night');
  const nightsOpt = optionValue('--nights');
  const packageOpt = optionValue('--package');
  const jsonOpt = args.includes('--json');
  const maxPriceOpt = optionValue('--max-price');
  const freshHoursOpt = optionValue('--fresh-hours');
  const maxOpt = optionValue('--max');
  const maxAgeOpt = optionValue('--max-age');
  const tripIdOpt = optionValue('--trip-id');
  const categoryOpt = optionValue('--category');
  const statusFilterOpt = optionValue('--status');
  const dirOpt = optionValue('--dir');
  const filesOpt = optionValue('--files');
  const noteOpt = optionValue('--note');
  const flightOpt = optionValue('--flight');
  const airlineOpt = optionValue('--airline');
  const airlineCodeOpt = optionValue('--airline-code');
  const fromOpt = optionValue('--from');
  const depTerminalOpt = optionValue('--dep-terminal');
  const depOpt = optionValue('--dep');
  const toOpt = optionValue('--to');
  const arrTerminalOpt = optionValue('--arr-terminal');
  const arrOpt = optionValue('--arr');
  const dateOpt = optionValue('--date');
  const bookedDateOpt = optionValue('--booked-date');
  const hotelNameOpt = optionValue('--name');
  const checkInOpt = optionValue('--check-in');
  const accessOpt = optionValue('--access');
  const startTimeOpt = optionValue('--start-time');

  // Filter out flags/options from args
  const optionsWithValues = new Set([
    '--dest',
    '--pax',
    '--plan-id',
    '--plan',
    '--state',
    '--goals',
    '--pace',
    '--assign',
    '--ref',
    '--book-by',
    '--selected',
    '--candidate',
    '--start',
    '--end',
    '--fixed',
    '--severity',
    '--types',
    '--source',
    '--flights',
    '--hotel-per-night',
    '--nights',
    '--package',
    '--max-price',
    '--fresh-hours',
    '--max',
    '--max-age',
    '--trip-id',
    '--category',
    '--status',
    '--limit',
    '--dir',
    '--files',
    '--note',
    '--flight',
    '--airline',
    '--airline-code',
    '--from',
    '--dep-terminal',
    '--dep',
    '--to',
    '--arr-terminal',
    '--arr',
    '--date',
    '--booked-date',
    '--name',
    '--check-in',
    '--access',
    '--duration',
    '--notes',
    '--start-time',
    '--zh',
    '--transit-zh',
    '--activities-zh-json',
  ]);
  const cleanArgs: string[] = [];
  for (let i = 0; i < args.length; i++) {
    const a = args[i];
    if (a.startsWith('--')) {
      if (optionsWithValues.has(a)) i++; // skip value
      continue;
    }
    cleanArgs.push(a);
  }

  if (planIdOpt && (planOpt || stateOpt)) {
    console.error('Error: --plan-id cannot be combined with --plan or --state');
    process.exit(1);
  }

  // Pre-load destination config from Turso DB so all sync loader APIs work
  const { loadDestinationConfigFromDb } = await import('../config/loader');
  await loadDestinationConfigFromDb();

  const sm = planIdOpt
    ? await StateManager.createFromPlanId(planIdOpt)
    : await StateManager.create(planOpt, stateOpt);

  try {
    switch (command) {
      case 'status': {
        showStatus(sm, { full });
        break;
      }

      case 'itinerary': {
        showItinerary(sm, destOpt);
        break;
      }

      case 'transport': {
        showTransport(sm, destOpt);
        break;
      }

      case 'bookings': {
        showBookings(sm, destOpt);
        break;
      }

      case 'scrape-package': {
        const [, url] = cleanArgs;
        if (!url) {
          console.error('Error: scrape-package requires <url>');
          console.error('Example: scrape-package "https://www.besttour.com.tw/itinerary/TYO05MM260211AM" --pax 2');
          process.exit(1);
        }

        const destination = sm.resolveDestination(destOpt);
        let pax = 2;
        if (paxOpt) {
          const paxResult = validatePositiveInt(paxOpt, '--pax');
          if (!paxResult.ok) {
            console.error(`Error: ${paxResult.error}`);
            process.exit(1);
          }
          pax = paxResult.value;
        }

        // Freshness check — skip scraping if Turso has recent data
        if (!dryRun && !args.includes('--force')) {
          try {
            const { checkFreshness } = await import('../services/turso-service');
            const sourceId = inferSourceIdFromUrl(url);
            const region = inferRegionFromDestination(destination);
            const freshness = await checkFreshness(sourceId, { region });
            if (freshness.recommendation === 'skip') {
              console.log(`\nTurso has fresh data for ${sourceId} (${freshness.ageHours?.toFixed(1)}h old, ${freshness.offerCount} offers).`);
              console.log('Use --force to scrape anyway, or query-offers to view existing data.');
              break;
            }
            if (freshness.recommendation === 'rescrape') {
              console.log(`\nTurso data for ${sourceId} is ${freshness.ageHours?.toFixed(1)}h old. Re-scraping...`);
            }
          } catch {
            // Turso not configured — proceed with scrape
          }
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
          await sm.saveWithTracking('import-offers', `${offers.length} offers from ${offers[0]?.source_id || 'unknown'}`);
          console.log('✅ Imported offers into process_3_4_packages.results.offers');

          // Auto-import to Turso (file still exists)
          try {
            const { importOffersFromFiles } = await import('../services/turso-service');
            const tursoResult = await importOffersFromFiles([tmpOut], {
              destination,
              region: inferRegionFromDestination(destination),
            });
            console.log(`  Turso: imported ${tursoResult.imported} offer(s)`);
          } catch (e) {
            console.warn(`  Turso auto-import skipped: ${(e as Error).message}`);
          }

          // Clean up temp file after both imports
          fs.unlinkSync(tmpOut);

          const best = offers[0]?.best_value?.date;
          if (best) {
            console.log(`\nNext action: npx ts-node src/cli/travel-update.ts select-offer ${offers[0].id} ${best}`);
          } else {
            console.log('\nNext action: review offers then run select-offer <offer-id> <date>');
          }
        } else {
          // Dry run — clean up temp file
          if (fs.existsSync(tmpOut)) fs.unlinkSync(tmpOut);
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

        // Validate date range
        const rangeResult = validateDateRange(startDate, endDate);
        if (!rangeResult.ok) {
          console.error(`Error: ${rangeResult.error}`);
          process.exit(1);
        }

        const reason = reasonParts.join(' ') || undefined;

        console.log(`\n📅 Setting dates: ${formatDate(startDate)} → ${formatDate(endDate)} (${rangeResult.value.days} days)`);
        if (reason) console.log(`   Reason: ${reason}`);

        if (!dryRun) {
          sm.setDateAnchor(startDate, endDate, reason);
          await sm.saveWithTracking('set-dates', `${startDate} ${endDate}`);
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
          await sm.saveWithTracking('update-offer', `${offerId} ${date} ${availability}`);
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
          const destination = sm.resolveDestination(destOpt);
          sm.selectOffer(offerId, date, !noPopulate);
          await sm.saveWithTracking('select-offer', offerId);
          console.log('✅ Offer selected');
          if (!noPopulate) {
            console.log('✅ P3 (transportation) and P4 (accommodation) populated from package');
          }

          // Turso booking sync handled automatically by save() → syncBookingsToDb()
        } else {
          console.log('🔸 DRY RUN - no changes saved');
        }

        if (verbose) showStatus(sm);
        break;
      }

      case 'scaffold-itinerary': {
        const destination = sm.resolveDestination(destOpt);
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
          await sm.saveWithTracking('scaffold-itinerary', `${destination} ${days.length} days`);
          console.log('\n✅ Itinerary scaffolded');
          console.log('\nNext action: Review day structure, then populate activities with /p5-itinerary');
        } else {
          console.log('\n🔸 DRY RUN - no changes saved');
        }

        break;
      }

      case 'mark-booked': {
        const destination = sm.resolveDestination(destOpt);
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

          await sm.saveWithTracking('mark-booked', destination);
          console.log('\n✅ Booking marked as confirmed');
          // Turso booking sync handled automatically by save() → syncBookingsToDb()

          console.log('\nNext action: Plan daily itinerary with scaffold-itinerary or /p5-itinerary');
        } else {
          console.log('\n🔸 DRY RUN - no changes saved');
        }

        break;
      }

      case 'populate-itinerary': {
        const destination = sm.resolveDestination(destOpt);
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

        const plannedAdds: Array<{ day: number; session: 'morning' | 'noon' | 'afternoon' | 'evening'; poiId: string; title: string }> = [];
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
          await sm.saveWithTracking('populate-itinerary', `${destination} ${plannedAdds.length} activities`);
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
          console.error('Example: set-airport-transfer arrival planned --selected "Limousine Bus|NRT T1 → Shiodome|85|3200|19:40 → ~21:05"');
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

        const destination = sm.resolveDestination(destOpt);
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
          await sm.saveWithTracking('set-airport-transfer', `${destination} ${direction}`);
          console.log('✅ Airport transfer updated');
        } else {
          console.log('🔸 DRY RUN - no changes saved');
        }

        if (verbose) showStatus(sm, { full });
        break;
      }

      case 'set-flight': {
        const [, direction] = cleanArgs;

        if (!direction || !['outbound', 'return'].includes(direction)) {
          console.error('Error: set-flight requires <outbound|return>');
          console.error('Example: set-flight outbound --dest kyoto_2026 --flight SL396 --airline "Thai Lion Air" --airline-code SL --from TPE --dep-terminal T1 --dep 09:00 --to KIX --arr-terminal T1 --arr 12:30 --date 2026-02-24');
          process.exit(1);
        }

        const destination = sm.resolveDestination(destOpt);
        const input: import('../state/repository').FlightLegInput = {};
        if (flightOpt) input.flightNumber = flightOpt;
        if (airlineOpt) input.airline = airlineOpt;
        if (airlineCodeOpt) input.airlineCode = airlineCodeOpt;
        if (fromOpt) input.departureCode = fromOpt;
        if (depTerminalOpt) input.departureTerminal = depTerminalOpt;
        if (depOpt) input.departureTime = depOpt;
        if (toOpt) input.arrivalCode = toOpt;
        if (arrTerminalOpt) input.arrivalTerminal = arrTerminalOpt;
        if (arrOpt) input.arrivalTime = arrOpt;
        if (dateOpt) input.date = dateOpt;
        if (bookedDateOpt) input.bookedDate = bookedDateOpt;

        console.log(`\n✈️  Setting flight leg:`);
        console.log(`   Destination: ${destination}`);
        console.log(`   Direction: ${direction}`);
        if (input.flightNumber) console.log(`   Flight: ${input.flightNumber}`);
        if (input.airline) console.log(`   Airline: ${input.airline} (${input.airlineCode ?? ''})`);
        if (input.departureCode) console.log(`   From: ${input.departureCode}${input.departureTerminal ? ' T' + input.departureTerminal : ''} ${input.departureTime ?? ''}`);
        if (input.arrivalCode) console.log(`   To: ${input.arrivalCode}${input.arrivalTerminal ? ' T' + input.arrivalTerminal : ''} ${input.arrivalTime ?? ''}`);
        if (input.date) console.log(`   Date: ${input.date}`);

        if (!dryRun) {
          sm.setFlightLeg(destination, direction as 'outbound' | 'return', input);
          await sm.saveWithTracking('set-flight', `${destination} ${direction} ${input.flightNumber ?? ''}`);
          console.log('✅ Flight leg updated');
        } else {
          console.log('🔸 DRY RUN - no changes saved');
        }
        break;
      }

      case 'set-hotel': {
        const destination = sm.resolveDestination(destOpt);
        const input: import('../state/repository').HotelInput = {};
        if (hotelNameOpt) input.name = hotelNameOpt;
        if (accessOpt) input.access = accessOpt.split('|').map(s => s.trim()).filter(Boolean);
        if (checkInOpt) input.checkIn = checkInOpt;
        if (noteOpt) input.notes = noteOpt;

        if (!input.name && !input.access && !input.checkIn && !input.notes) {
          console.error('Error: set-hotel requires at least one of --name, --access, --check-in, --note');
          console.error('Example: set-hotel --dest kyoto_2026 --name "APA Hotel Kyoto Ekimae" --check-in 2026-02-24 --access "JR Kyoto Station 3min"');
          process.exit(1);
        }

        console.log(`\n🏨 Setting hotel:`);
        console.log(`   Destination: ${destination}`);
        if (input.name) console.log(`   Name: ${input.name}`);
        if (input.checkIn) console.log(`   Check-in: ${input.checkIn}`);
        if (input.access) console.log(`   Access: ${input.access.join(' | ')}`);
        if (input.notes) console.log(`   Notes: ${input.notes}`);

        if (!dryRun) {
          sm.setHotel(destination, input);
          await sm.saveWithTracking('set-hotel', `${destination} ${input.name ?? ''}`);
          console.log('✅ Hotel updated');
        } else {
          console.log('🔸 DRY RUN - no changes saved');
        }
        break;
      }

      case 'delete-activity':
      case 'remove-activity': {
        const [, dayStr, sessionArg2, activityArg] = cleanArgs;
        if (!dayStr || !sessionArg2 || !activityArg) {
          console.error('Usage: delete-activity <day> <session> <activity_id_or_title>');
          console.error('Example: delete-activity 2 morning "teamLab Borderless"');
          process.exit(1);
        }
        const dayResult2 = validatePositiveInt(dayStr, '<day>');
        if (!dayResult2.ok) {
          console.error(`Error: ${dayResult2.error}`);
          process.exit(1);
        }
        const dayNum2 = dayResult2.value;
        const validSessions2 = ['morning', 'noon', 'afternoon', 'evening'];
        if (!validSessions2.includes(sessionArg2)) {
          console.error(`Error: <session> must be one of: morning | noon | afternoon | evening`);
          process.exit(1);
        }
        const dest2 = sm.resolveDestination(destOpt);
        // Resolve activity within the specific day+session to avoid title-collision false rejects
        const day2 = sm.getDay(dest2, dayNum2);
        if (!day2) {
          console.error(`Error: Day ${dayNum2} not found in ${dest2}`);
          process.exit(1);
        }
        const sessionObj2 = day2[sessionArg2] as { activities?: Array<string | Record<string, unknown>> } | undefined;
        const acts2 = sessionObj2?.activities ?? [];
        const searchLower2 = activityArg.toLowerCase();
        const matchAct2 = acts2.find((a: unknown) => {
          if (typeof a === 'string') return a.toLowerCase().includes(searchLower2);
          const r = a as Record<string, unknown>;
          return r.id === activityArg || (typeof r.title === 'string' && r.title.toLowerCase().includes(searchLower2));
        });
        if (!matchAct2) {
          console.error(`Error: Activity "${activityArg}" not found in Day ${dayNum2} ${sessionArg2}`);
          const actList = acts2.map((a: unknown) => typeof a === 'string' ? `  "${a}"` : `  [${(a as Record<string,unknown>).id}] ${(a as Record<string,unknown>).title}`);
          if (actList.length) console.error('Activities in this session:\n' + actList.join('\n'));
          process.exit(1);
        }
        const matchTitle = typeof matchAct2 === 'string' ? matchAct2 : (matchAct2 as Record<string, unknown>).title as string;
        const matchId = typeof matchAct2 === 'string' ? matchAct2 : (matchAct2 as Record<string, unknown>).id as string;
        console.log(`\n🗑️  Deleting activity:`);
        console.log(`   Day ${dayNum2} ${sessionArg2}: "${matchTitle}"`);
        if (!dryRun) {
          await sm.dispatch({
            type: 'remove_activity',
            destination: dest2,
            dayNumber: dayNum2,
            session: sessionArg2 as SessionType,
            activityId: matchId,
          });
          await sm.saveWithTracking('delete-activity', `D${dayNum2}/${sessionArg2}/${matchTitle}`);
          console.log('✅ Activity deleted');
        } else {
          console.log('🔸 DRY RUN - no changes saved');
        }
        break;
      }

      case 'set-activity-time': {
        const [, dayStr, session, activity] = cleanArgs;
        if (!dayStr || !session || !activity) {
          console.error('Error: set-activity-time requires <day> <session> <activity>');
          console.error('Example: set-activity-time 5 afternoon "Hotel checkout" --start 11:00 --fixed true');
          process.exit(1);
        }

        const dayResult = validatePositiveInt(dayStr, '<day>');
        if (!dayResult.ok) {
          console.error(`Error: ${dayResult.error}`);
          process.exit(1);
        }
        const dayNumber = dayResult.value;

        const validSessions = ['morning', 'noon', 'afternoon', 'evening'];
        if (!validSessions.includes(session)) {
          console.error('Error: <session> must be one of: morning | noon | afternoon | evening');
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

        // Validate time formats
        let validatedStart: string | undefined;
        let validatedEnd: string | undefined;
        if (startOpt) {
          const startResult = validateTime(startOpt, '--start');
          if (!startResult.ok) {
            console.error(`Error: ${startResult.error}`);
            process.exit(1);
          }
          validatedStart = startResult.value;
        }
        if (endOpt) {
          const endResult = validateTime(endOpt, '--end');
          if (!endResult.ok) {
            console.error(`Error: ${endResult.error}`);
            process.exit(1);
          }
          validatedEnd = endResult.value;
        }

        const destination = sm.resolveDestination(destOpt);

        console.log(`\n⏱️  Setting activity time:`);
        console.log(`   Destination: ${destination}`);
        console.log(`   Day ${dayNumber} ${session}: "${activity}"`);
        if (validatedStart) console.log(`   Start: ${validatedStart}`);
        if (validatedEnd) console.log(`   End: ${validatedEnd}`);
        if (isFixed !== undefined) console.log(`   Fixed: ${isFixed}`);

        if (!dryRun) {
          sm.setActivityTime(destination, dayNumber, session as any, activity, {
            start_time: validatedStart,
            end_time: validatedEnd,
            is_fixed_time: isFixed,
          });
          await sm.saveWithTracking('set-activity-time', `D${dayNumber} ${session} ${activity}`);
          console.log('✅ Activity time updated');
        } else {
          console.log('🔸 DRY RUN - no changes saved');
        }

        if (verbose) showStatus(sm, { full });
        break;
      }

      case 'set-tod-focus':
      case 'set-session-focus': {
        // set-tod-focus <day> <session> "<focus_text>" [--plan-id <id>]
        const [, dayStr, sessionArg3, focusText] = cleanArgs;
        if (!dayStr || !sessionArg3) {
          console.error('Usage: set-tod-focus <day> <session> "<focus_text>"');
          console.error('Example: set-tod-focus 2 morning "Kitano Tenmangu → Kinkaku-ji"');
          process.exit(1);
        }
        const dayResult3 = validatePositiveInt(dayStr, '<day>');
        if (!dayResult3.ok) {
          console.error(`Error: ${dayResult3.error}`);
          process.exit(1);
        }
        const dayNum3 = dayResult3.value;
        const validSessions3 = ['morning', 'noon', 'afternoon', 'evening'];
        if (!validSessions3.includes(sessionArg3)) {
          console.error(`Error: <session> must be one of: morning | noon | afternoon | evening`);
          process.exit(1);
        }
        const dest3 = sm.resolveDestination(destOpt);
        if (!dryRun) {
          await sm.dispatch({
            type: 'set_session_focus',
            destination: dest3,
            dayNumber: dayNum3,
            session: sessionArg3 as SessionType,
            focus: focusText ?? null,
          });
          await sm.saveWithTracking('set-tod-focus', `D${dayNum3}/${sessionArg3}`);
          console.log(`✅ Focus set for D${dayNum3} ${sessionArg3}: "${focusText ?? '(cleared)'}"`);
        } else {
          console.log(`🔸 DRY RUN — would set D${dayNum3} ${sessionArg3} focus: "${focusText}"`);
        }
        break;
      }

      case 'set-tod-time-range':
      case 'set-session-time-range': {
        const [, dayStr, session] = cleanArgs;
        if (!dayStr || !session) {
          console.error('Error: set-session-time-range requires <day> <session>');
          console.error('Example: set-session-time-range 5 afternoon --start 11:00 --end 14:45');
          process.exit(1);
        }

        const dayResult = validatePositiveInt(dayStr, '<day>');
        if (!dayResult.ok) {
          console.error(`Error: ${dayResult.error}`);
          process.exit(1);
        }
        const dayNumber = dayResult.value;

        const validSessions = ['morning', 'noon', 'afternoon', 'evening'];
        if (!validSessions.includes(session)) {
          console.error('Error: <session> must be one of: morning | noon | afternoon | evening');
          process.exit(1);
        }

        if (!startOpt || !endOpt) {
          console.error('Error: set-session-time-range requires --start HH:MM and --end HH:MM');
          process.exit(1);
        }

        // Validate time formats
        const startTimeResult = validateTime(startOpt, '--start');
        if (!startTimeResult.ok) {
          console.error(`Error: ${startTimeResult.error}`);
          process.exit(1);
        }
        const endTimeResult = validateTime(endOpt, '--end');
        if (!endTimeResult.ok) {
          console.error(`Error: ${endTimeResult.error}`);
          process.exit(1);
        }

        const destination = sm.resolveDestination(destOpt);
        console.log(`\n🕒 Setting session time range:`);
        console.log(`   Destination: ${destination}`);
        console.log(`   Day ${dayNumber} ${session}: ${startTimeResult.value} → ${endTimeResult.value}`);

        if (!dryRun) {
          sm.setSessionTimeRange(destination, dayNumber, session as any, startTimeResult.value, endTimeResult.value);
          await sm.saveWithTracking('set-session-time-range', `D${dayNumber} ${session}`);
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

        const dayResult = validatePositiveInt(dayStr, '<day>');
        if (!dayResult.ok) {
          console.error(`Error: ${dayResult.error}`);
          process.exit(1);
        }
        const dayNumber = dayResult.value;

        const validSessions = ['morning', 'noon', 'afternoon', 'evening'];
        if (!validSessions.includes(session)) {
          console.error('Error: <session> must be one of: morning | noon | afternoon | evening');
          process.exit(1);
        }

        const validStatuses = ['not_required', 'pending', 'booked', 'waitlist'];
        if (!validStatuses.includes(status)) {
          console.error('Error: <status> must be one of: not_required | pending | booked | waitlist');
          process.exit(1);
        }

        // Validate book-by date if provided
        let validatedBookBy: string | undefined;
        if (bookByOpt) {
          const bookByResult = validateIsoDate(bookByOpt, '--book-by');
          if (!bookByResult.ok) {
            console.error(`Error: ${bookByResult.error}`);
            process.exit(1);
          }
          validatedBookBy = bookByResult.value;
        }

        const destination = sm.resolveDestination(destOpt);

        console.log(`\n🎫 Setting activity booking status:`);
        console.log(`   Destination: ${destination}`);
        console.log(`   Day ${dayNumber} ${session}: "${activity}"`);
        console.log(`   Status: ${status}`);
        if (refOpt) console.log(`   Reference: ${refOpt}`);
        if (validatedBookBy) console.log(`   Book by: ${validatedBookBy}`);

        if (!dryRun) {
          sm.setActivityBookingStatus(
            destination,
            dayNumber,
            session as 'morning' | 'noon' | 'afternoon' | 'evening',
            activity,
            status as 'not_required' | 'pending' | 'booked' | 'waitlist',
            refOpt,
            validatedBookBy
          );
          await sm.saveWithTracking('set-activity-booking', `D${dayNumber} ${session} ${activity}`);
          console.log('✅ Activity booking status updated');
        } else {
          console.log('🔸 DRY RUN - no changes saved');
        }

        break;
      }

      case 'set-route-segment': {
        const [, dayStr, sortOrderStr, fromPlace, toPlace, mode] = cleanArgs;

        if (!dayStr || !sortOrderStr || !fromPlace || !toPlace || !mode) {
          console.error('Error: set-route-segment requires <day> <sort_order> <from> <to> <mode>');
          console.error('Example: set-route-segment 1 0 "關渡" "嘟嘟房桃園機場貨運1站" driving --duration 45');
          process.exit(1);
        }

        const dayResult = validatePositiveInt(dayStr, '<day>');
        if (!dayResult.ok) { console.error(`Error: ${dayResult.error}`); process.exit(1); }
        const dayNumber = dayResult.value;

        const sortOrder = parseInt(sortOrderStr, 10);
        if (isNaN(sortOrder) || sortOrder < 0) {
          console.error('Error: <sort_order> must be a non-negative integer (0-based)');
          process.exit(1);
        }

        const validModes = ['transit', 'walking', 'driving'];
        if (!validModes.includes(mode)) {
          console.error('Error: <mode> must be one of: transit | walking | driving');
          process.exit(1);
        }

        const durationMin = durationOpt ? parseInt(durationOpt, 10) : undefined;
        if (durationOpt && isNaN(durationMin!)) {
          console.error('Error: --duration must be a number (minutes)');
          process.exit(1);
        }

        const destination = sm.resolveDestination(destOpt);

        console.log(`\n🗺️  Setting route segment:`);
        console.log(`   Destination: ${destination}`);
        console.log(`   Day ${dayNumber} slot ${sortOrder}: ${fromPlace} → ${toPlace} (${mode}${durationMin != null ? `, ${durationMin} min` : ''}${startTimeOpt ? `, start ${startTimeOpt}` : ''})`);
        if (notesOpt) console.log(`   Notes: ${notesOpt}`);

        if (!dryRun) {
          sm.setRouteSegment(destination, dayNumber, sortOrder, fromPlace, toPlace, mode as 'transit' | 'walking' | 'driving', durationMin, notesOpt ?? undefined, startTimeOpt ?? undefined);
          await sm.saveWithTracking('set-route-segment', `D${dayNumber} slot${sortOrder} ${fromPlace}→${toPlace}`);
          console.log('✅ Route segment updated');
        } else {
          console.log('🔸 DRY RUN - no changes saved');
        }

        break;
      }

      case 'set-route-segments-bulk': {
        // set-route-segments-bulk <day> --json '[{"from":"A","to":"B","mode":"walking","duration":5},...]'
        const [, dayStr] = cleanArgs;

        if (!dayStr) {
          console.error('Error: set-route-segments-bulk requires <day> --json \'[...]\'');
          console.error('  Each segment: {"from":"place","to":"place","mode":"transit|walking|driving","duration":N,"notes":"...","start_time":"HH:MM"}');
          console.error('Example: set-route-segments-bulk 4 --json \'[{"from":"hotel","to":"京都駅","mode":"walking","duration":3}]\'');
          process.exit(1);
        }

        const dayResult = validatePositiveInt(dayStr, '<day>');
        if (!dayResult.ok) {
          console.error(`Error: ${dayResult.error}`);
          process.exit(1);
        }
        const dayNumber = dayResult.value;

        const jsonOpt = optionValue('--json');
        if (!jsonOpt) {
          console.error('Error: --json is required with a JSON array of route segments');
          process.exit(1);
        }

        let segments: Array<{ from: string; to: string; mode: 'transit' | 'walking' | 'driving'; duration?: number; notes?: string; start_time?: string }>;
        try {
          segments = JSON.parse(jsonOpt);
          if (!Array.isArray(segments)) throw new Error('Not an array');
          for (const s of segments) {
            if (!s.from || !s.to || !s.mode) throw new Error('Each segment needs from, to, mode');
            if (!['transit', 'walking', 'driving'].includes(s.mode)) throw new Error(`Invalid mode: ${s.mode}`);
          }
        } catch (e) {
          console.error(`Error: Invalid --json: ${(e as Error).message}`);
          process.exit(1);
        }

        const destination = sm.resolveDestination(destOpt);
        console.log(`\n🛤️  Replacing all route segments for Day ${dayNumber}:`);
        console.log(`   Destination: ${destination}`);
        console.log(`   ${segments.length} segments`);
        for (const s of segments) {
          console.log(`   ${s.from} → ${s.to} (${s.mode}${s.duration ? `, ${s.duration}min` : ''})`);
        }

        if (!dryRun) {
          sm.setRouteSegmentsBulk(destination, dayNumber, segments);
          await sm.saveWithTracking('set-route-segments-bulk', `D${dayNumber} ${segments.length} segments`);
          console.log('✅ Route segments replaced');
        } else {
          console.log('🔸 DRY RUN - no changes saved');
        }

        break;
      }

      case 'set-activity-title': {
        const [, dayStr, session, activity, newTitle] = cleanArgs;

        if (!dayStr || !session || !activity || !newTitle) {
          console.error('Error: set-activity-title requires <day> <session> <activity> <new_title>');
          console.error('Example: set-activity-title 2 morning "Fushimi Inari" "金閣寺 (Kinkaku-ji)"');
          process.exit(1);
        }

        const dayResult = validatePositiveInt(dayStr, '<day>');
        if (!dayResult.ok) {
          console.error(`Error: ${dayResult.error}`);
          process.exit(1);
        }
        const dayNumber = dayResult.value;

        const validSessions = ['morning', 'noon', 'afternoon', 'evening'];
        if (!validSessions.includes(session)) {
          console.error('Error: <session> must be one of: morning | noon | afternoon | evening');
          process.exit(1);
        }

        if (!newTitle.trim()) {
          console.error('Error: <new_title> cannot be empty');
          process.exit(1);
        }

        const destination = sm.resolveDestination(destOpt);

        console.log(`\n✏️  Renaming activity:`);
        console.log(`   Destination: ${destination}`);
        console.log(`   Day ${dayNumber} ${session}: "${activity}"`);
        console.log(`   New title: "${newTitle}"`);

        if (!dryRun) {
          sm.setActivityTitle(
            destination,
            dayNumber,
            session as 'morning' | 'noon' | 'afternoon' | 'evening',
            activity,
            newTitle
          );
          await sm.saveWithTracking('set-activity-title', `D${dayNumber} ${session} ${activity}`);
          console.log('✅ Activity title updated');
        } else {
          console.log('🔸 DRY RUN - no changes saved');
        }

        break;
      }

      case 'set-day-theme': {
        const [, dayStr, themeArg] = cleanArgs;

        if (!dayStr) {
          console.error('Error: set-day-theme requires <day> and --zh "<title>"');
          console.error('Example: set-day-theme 1 --zh "抵達京都"');
          console.error('Example: set-day-theme 2 "Kinkaku-ji full day" --zh "金閣寺・伏見稲荷"');
          process.exit(1);
        }

        const dayResult = validatePositiveInt(dayStr, '<day>');
        if (!dayResult.ok) {
          console.error(`Error: ${dayResult.error}`);
          process.exit(1);
        }
        const dayNumber = dayResult.value;
        const themeZhOpt = optionValue('--zh');

        // themeArg is optional (update only ZH if omitted)
        const destination = sm.resolveDestination(destOpt);

        console.log(`\n🏷️  Setting day theme:`);
        console.log(`   Destination: ${destination}, Day ${dayNumber}`);
        if (themeArg) console.log(`   Theme (EN): "${themeArg}"`);
        if (themeZhOpt) console.log(`   Theme (ZH): "${themeZhOpt}"`);

        if (!dryRun) {
          sm.setDayTheme(destination, dayNumber, themeArg ?? null, themeZhOpt ?? undefined);
          await sm.saveWithTracking('set-day-theme', `D${dayNumber}`);
          console.log('✅ Day theme updated');
        } else {
          console.log('🔸 DRY RUN - no changes saved');
        }

        break;
      }

      case 'set-tod-zh':
      case 'set-session-zh': {
        // set-session-zh <day> <session> [--zh "focus_zh"] [--transit-zh "transit_notes_zh"] [--activities-zh-json '[...]'] [--meals-zh-json '[...]']
        const [, dayStr, sessionArg] = cleanArgs;

        if (!dayStr || !sessionArg) {
          console.error('Error: set-session-zh requires <day> <session>');
          console.error('  --zh "Chinese focus text"');
          console.error('  --transit-zh "Chinese transit notes"');
          console.error('  --activities-zh-json \'["act1","act2"]\'');
          console.error('  --meals-zh-json \'["lunch: 拉麵","dinner: 壽司"]\'');
          console.error('Example: set-session-zh 2 morning --zh "金閣寺・可選龍安寺" --transit-zh "地鐵→北大路→公車→金閣寺道"');
          process.exit(1);
        }

        const dayResult = validatePositiveInt(dayStr, '<day>');
        if (!dayResult.ok) {
          console.error(`Error: ${dayResult.error}`);
          process.exit(1);
        }
        const sessionDay = dayResult.value;
        const sessionType = sessionArg as 'morning' | 'noon' | 'afternoon' | 'evening';
        if (!['morning', 'noon', 'afternoon', 'evening'].includes(sessionType)) {
          console.error(`Error: session must be morning, noon, afternoon, or evening`);
          process.exit(1);
        }

        const focusZhOpt = optionValue('--zh') ?? undefined;
        const transitZhOpt = optionValue('--transit-zh') ?? undefined;
        const activitiesZhJsonOpt = optionValue('--activities-zh-json') ?? undefined;
        const mealsZhJsonOpt = optionValue('--meals-zh-json') ?? undefined;
        let activitiesZh: string[] | undefined;
        if (activitiesZhJsonOpt) {
          try {
            activitiesZh = JSON.parse(activitiesZhJsonOpt);
          } catch {
            console.error('Error: --activities-zh-json must be a valid JSON array of strings');
            process.exit(1);
          }
        }
        let mealsZh: string[] | undefined;
        if (mealsZhJsonOpt) {
          try {
            mealsZh = JSON.parse(mealsZhJsonOpt);
          } catch {
            console.error('Error: --meals-zh-json must be a valid JSON array of strings');
            process.exit(1);
          }
        }

        const destination = sm.resolveDestination(destOpt);
        console.log(`\n🌏 Setting session ZH content:`);
        console.log(`   Destination: ${destination}, Day ${sessionDay}, ${sessionType}`);
        if (focusZhOpt) console.log(`   focus_zh: "${focusZhOpt}"`);
        if (transitZhOpt) console.log(`   transit_notes_zh: "${transitZhOpt}"`);
        if (activitiesZh) console.log(`   activities_zh: ${activitiesZh.length} items`);
        if (mealsZh) console.log(`   meals_zh: ${mealsZh.length} items`);

        if (!dryRun) {
          sm.setSessionZhContent(destination, sessionDay, sessionType, focusZhOpt, transitZhOpt, activitiesZh, mealsZh);
          await sm.saveWithTracking('set-session-zh', `D${sessionDay}/${sessionType}`);
          console.log('✅ Session ZH content updated');
        } else {
          console.log('🔸 DRY RUN - no changes saved');
        }

        break;
      }

      case 'swap-days': {
        const [, dayAStr, dayBStr] = cleanArgs;
        if (!dayAStr || !dayBStr) {
          console.error('Error: swap-days requires <dayA> <dayB>');
          console.error('Example: swap-days 2 3');
          process.exit(1);
        }

        const dayAResult = validatePositiveInt(dayAStr, '<dayA>');
        const dayBResult = validatePositiveInt(dayBStr, '<dayB>');
        if (!dayAResult.ok) {
          console.error(`Error: ${dayAResult.error}`);
          process.exit(1);
        }
        if (!dayBResult.ok) {
          console.error(`Error: ${dayBResult.error}`);
          process.exit(1);
        }

        const dayA = dayAResult.value;
        const dayB = dayBResult.value;

        if (dayA === dayB) {
          console.error('Error: dayA and dayB must be different');
          process.exit(1);
        }

        const destination = sm.resolveDestination(destOpt);
        console.log(`\n🔄 Swapping days:`);
        console.log(`   Destination: ${destination}`);
        console.log(`   Day ${dayA} ↔ Day ${dayB}`);

        if (!dryRun) {
          sm.swapDays(destination, dayA, dayB);
          await sm.saveWithTracking('swap-days', `D${dayA} ↔ D${dayB}`);
          console.log('✅ Days swapped successfully');
        } else {
          console.log('🔸 DRY RUN - no changes saved');
        }
        break;
      }

      case 'validate-itinerary': {
        const destination = sm.resolveDestination(destOpt);
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

        const daySummaries = buildDaySummaries(days);
        const result = defaultValidator.validate(daySummaries, new Date());

        const threshold: IssueSeverity = parseSeverity(severityOpt);
        const filtered = {
          ...result,
          issues: result.issues.filter((i) => severityRank(i.severity) >= severityRank(threshold)),
        };

        if (jsonOpt) {
          console.log(JSON.stringify(filtered, null, 2));
        } else {
          printValidationResult(destination, filtered, threshold);
        }

        process.exitCode = filtered.valid ? 0 : 1;
        break;
      }

      case 'search-offers': {
        const destination = destOpt || optionValue('--dest');
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

        console.log(`\n🔎 search-offers (${destination})`);
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
        break;
      }

      case 'compare-offers': {
        const region = optionValue('--region');
        if (!region) {
          console.error('Error: compare-offers requires --region <name>');
          console.error('Example: compare-offers --region osaka');
          process.exit(1);
        }

        const filterDate = optionValue('--date');
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
        break;
      }

      case 'view-prices': {
        if (!flightsOpt) {
          console.error('Error: view-prices requires --flights <file>');
          console.error('Example: view-prices --flights scrapes/date-range-prices.json --hotel-per-night 3000 --nights 4');
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

        const hotelPerNight = hotelPerNightOpt ? parseInt(hotelPerNightOpt, 10) : undefined;
        const nights = nightsOpt ? parseInt(nightsOpt, 10) : undefined;
        const packagePrice = packageOpt ? parseInt(packageOpt, 10) : undefined;

        showPriceComparison(flightsOpt, { hotelPerNight, nights, packagePrice, pax, json: jsonOpt });
        break;
      }

      case 'import-offers': {
        const destination = sm.resolveDestination(destOpt);
        let filePaths: string[];
        if (filesOpt) {
          filePaths = filesOpt.split(',').map(s => s.trim()).filter(Boolean);
        } else {
          const dir = dirOpt || 'scrapes';
          filePaths = fs.existsSync(dir)
            ? fs.readdirSync(dir)
              .filter(f => f.endsWith('.json') && !f.includes('schema') && !f.includes('destinations') && !f.includes('ota-sources'))
              .map(f => path.join(dir, f))
            : [];
        }
        if (filePaths.length === 0) {
          console.error('No JSON files found. Use --dir <path> or --files <csv>.');
          process.exit(1);
        }
        console.log(`Importing offers for ${destination} from ${filePaths.length} file(s)${dryRun ? ' (dry-run)' : ''}...`);
        const counts = await sm.importScrapedOffers(destination, filePaths, {
          start: startOpt,
          end: endOpt,
          pax: paxOpt ? parseInt(paxOpt, 10) : 2,
          dryRun,
          note: noteOpt,
        });
        if (Object.keys(counts).length === 0) {
          console.log('No offers imported (no matching files or all offers filtered out).');
        } else {
          for (const [src, count] of Object.entries(counts)) {
            console.log(`  ${src}: ${count} offer(s)`);
          }
          if (!dryRun) {
            console.log(`Saved to Turso (plan_offers).`);
          }
        }
        break;
      }

      case 'query-offers': {
        const { queryOffers, printTursoOfferTable } = await import('../services/turso-service');
        const results = await queryOffers({
          planId: planIdOpt,
          destination: destOpt,
          region: optionValue('--region'),
          start: startOpt,
          end: endOpt,
          sources: sourceOpt?.split(','),
          type: typesOpt as 'package' | 'flight' | 'hotel' | undefined,
          maxPrice: maxPriceOpt ? parseInt(maxPriceOpt, 10) : undefined,
          freshHours: freshHoursOpt ? parseInt(freshHoursOpt, 10) : undefined,
          limit: maxOpt ? parseInt(maxOpt, 10) : undefined,
        });
        if (jsonOpt) {
          console.log(JSON.stringify(results, null, 2));
        } else {
          printTursoOfferTable(results);
        }
        break;
      }

      case 'check-freshness': {
        const source = sourceOpt || optionValue('--source');
        if (!source) {
          console.error('Error: check-freshness requires --source <id>');
          console.error('Example: check-freshness --source besttour --region kansai');
          process.exit(1);
        }
        const { checkFreshness } = await import('../services/turso-service');
        const result = await checkFreshness(source, {
          region: optionValue('--region'),
          start: startOpt,
          end: endOpt,
          maxAgeHours: maxAgeOpt ? parseInt(maxAgeOpt, 10) : 24,
          planId: planIdOpt,
          destination: destOpt,
        });
        console.log(`Source:  ${source}`);
        if (result.region) console.log(`Region:  ${result.region}`);
        console.log(`Result:  ${result.recommendation}`);
        if (result.ageHours !== null) console.log(`  Age:     ${result.ageHours.toFixed(1)}h`);
        console.log(`  Offers:  ${result.offerCount}`);
        break;
      }

      case 'sync-bookings': {
        const { syncBookingsFromPlanJson } = await import('../services/turso-service');
        const { TursoRepository } = await import('../state/turso-repository');
        const effectivePlanId = planIdOpt || 'tokyo-2026';
        console.log(`Syncing bookings from plan "${effectivePlanId}"...`);
        const repo = await TursoRepository.create(effectivePlanId);
        const plan = repo.getPlan() as unknown as Record<string, unknown>;
        const { toDestSlug } = await import('../utils/plan-id');
        const effectiveTripId = tripIdOpt || toDestSlug(effectivePlanId);
        const syncResult = await syncBookingsFromPlanJson(plan, effectiveTripId, {
          dryRun: dryRun,
        });
        if (syncResult.warnings.length > 0) {
          console.warn('Warnings:');
          for (const w of syncResult.warnings) console.warn(`  - ${w}`);
        }
        console.log(`${dryRun ? 'Would sync' : 'Synced'} ${syncResult.synced} bookings to Turso.`);
        break;
      }

      case 'query-bookings': {
        const { queryBookings, printBookingsTable } = await import('../services/turso-service');
        const results = await queryBookings({
          tripId: tripIdOpt,
          destination: destOpt,
          category: categoryOpt as 'package' | 'transfer' | 'activity' | undefined,
          status: statusFilterOpt,
          limit: maxOpt ? parseInt(maxOpt, 10) : undefined,
        });
        if (jsonOpt) {
          console.log(JSON.stringify(results, null, 2));
        } else {
          printBookingsTable(results);
        }
        break;
      }

      case 'check-booking-integrity': {
        const { checkBookingIntegrity } = await import('../services/turso-service');
        const { toDestSlug } = await import('../utils/plan-id');
        const effectivePlanId = planIdOpt || 'tokyo-2026';
        const effectiveTripId = tripIdOpt || toDestSlug(effectivePlanId);
        const plan = sm.getPlan() as unknown as Record<string, unknown>;
        console.log(`Checking booking integrity (plan "${effectivePlanId}" vs Turso DB)...`);
        const integrity = await checkBookingIntegrity(plan, effectiveTripId);
        console.log(`\nResults:`);
        console.log(`  Matches:    ${integrity.matches}`);
        console.log(`  Mismatches: ${integrity.mismatches.length}`);
        console.log(`  Plan-only:  ${integrity.planOnly.length}`);
        console.log(`  DB-only:    ${integrity.dbOnly.length}`);
        if (integrity.mismatches.length > 0) {
          console.log('\nMismatches:');
          for (const m of integrity.mismatches) console.log(`  - ${m}`);
        }
        if (integrity.planOnly.length > 0) {
          console.log('\nPlan-only (not in DB):');
          for (const p of integrity.planOnly) console.log(`  - ${p}`);
        }
        if (integrity.dbOnly.length > 0) {
          console.log('\nDB-only (not in plan):');
          for (const d of integrity.dbOnly) console.log(`  - ${d}`);
        }
        break;
      }

      case 'fetch-weather': {
        if (args.includes('--all')) {
          const plan = sm.getPlan();
          const destinations = Object.keys(plan.destinations || {});
          let totalUpdated = 0;
          for (const d of destinations) {
            const destObj = plan.destinations[d] as Record<string, unknown> | undefined;
            const p5 = destObj?.process_5_daily_itinerary as Record<string, unknown> | undefined;
            const days = p5?.days as Array<Record<string, unknown>> | undefined;
            if (!days || days.length === 0) { console.log(`  Skipping ${d} (no itinerary days)`); continue; }
            const firstDate = days[0].date as string;
            const lastDate = days[days.length - 1].date as string;
            const { fetchWeather } = require('../services/weather-service');
            try {
              const forecasts = await fetchWeather(firstDate, lastDate, d);
              if (forecasts.length === 0) { console.log(`  ${d}: outside 16-day window`); continue; }
              for (let i = 0; i < days.length && i < forecasts.length; i++) {
                sm.setDayWeather(d, days[i].day_number as number, forecasts[i]);
              }
              console.log(`  ${d}: updated ${forecasts.length} day(s)`);
              totalUpdated += forecasts.length;
            } catch (e: any) {
              console.warn(`  ${d}: failed — ${e.message}`);
            }
          }
          if (totalUpdated > 0) await sm.saveWithTracking('fetch-weather', 'all destinations');
          break;
        }

        const dest = sm.resolveDestination(destOpt);
        const plan = sm.getPlan();
        const destObj = plan.destinations[dest] as Record<string, unknown> | undefined;
        if (!destObj) {
          console.error(`Destination not found: ${dest}`);
          process.exit(1);
        }

        const p5 = destObj.process_5_daily_itinerary as Record<string, unknown> | undefined;
        const days = p5?.days as Array<Record<string, unknown>> | undefined;
        if (!days || days.length === 0) {
          console.error('No itinerary days found. Run scaffold-itinerary first.');
          process.exit(1);
        }

        const firstDate = days[0].date as string;
        const lastDate = days[days.length - 1].date as string;

        const { fetchWeather } = require('../services/weather-service');
        const forecasts = await fetchWeather(firstDate, lastDate, dest);

        if (forecasts.length === 0) {
          console.log('No forecast data available (dates may be outside 16-day window).');
          break;
        }

        for (let i = 0; i < days.length && i < forecasts.length; i++) {
          const dayNum = days[i].day_number as number;
          sm.setDayWeather(dest, dayNum, forecasts[i]);
        }

        await sm.saveWithTracking('fetch-weather', dest);

        console.log(`Weather updated for ${forecasts.length} day(s) in ${dest}:`);
        for (let i = 0; i < forecasts.length; i++) {
          const f = forecasts[i];
          const feelsLike = (f.feels_like_low_c != null && f.feels_like_high_c != null)
            ? ` (體感 ${f.feels_like_low_c}–${f.feels_like_high_c}°C)`
            : '';
          console.log(`  Day ${days[i].day_number}: ${f.weather_label} ${f.temp_low_c}–${f.temp_high_c}°C${feelsLike}, Rain: ${f.precipitation_pct}%`);
        }
        break;
      }

      // =================================================================
      // Operation Run Tracking Commands
      // =================================================================

      case 'run-status': {
        const runId = cleanArgs[1];
        const planId = sm.getPlanId();
        const { getOperationRun } = require('../services/turso-service');

        const run = await getOperationRun(planId, runId || undefined);
        if (!run) {
          console.log(runId ? `No operation found with run_id: ${runId}` : 'No operations found for this plan.');
          break;
        }
        console.log('\n📋 Operation Run Details');
        console.log('─'.repeat(50));
        console.log(`  Run ID:       ${run.run_id}`);
        console.log(`  Plan ID:      ${run.plan_id}`);
        console.log(`  Command:      ${run.command_type}`);
        if (run.command_summary) console.log(`  Summary:      ${run.command_summary}`);
        console.log(`  Status:       ${run.status === 'completed' ? '✅' : run.status === 'failed' ? '❌' : '⏳'} ${run.status}`);
        console.log(`  Version:      ${run.version_before}${run.version_after != null ? ` → ${run.version_after}` : ''}`);
        console.log(`  Started:      ${run.started_at}`);
        if (run.completed_at) console.log(`  Completed:    ${run.completed_at}`);
        if (run.error_message) console.log(`  Error:        ${run.error_message}`);
        console.log('');
        break;
      }

      case 'run-list': {
        const planId = sm.getPlanId();
        const limitRaw = parseInt(optionValue('--limit') || '20', 10);
        const { listOperationRuns } = require('../services/turso-service');

        const runs = await listOperationRuns(planId, {
          status: statusFilterOpt || undefined,
          limit: limitRaw || 20,
        });

        if (runs.length === 0) {
          console.log('No operations found.');
          break;
        }

        console.log(`\n📋 Recent Operations (${runs.length} results)`);
        console.log('─'.repeat(90));
        console.log(
          [
            'Status'.padEnd(5),
            'Command'.padEnd(22),
            'Summary'.padEnd(30),
            'Ver'.padEnd(7),
            'Started'.padEnd(20),
          ].join(' │ ')
        );
        console.log('─'.repeat(90));

        for (const run of runs) {
          const icon = run.status === 'completed' ? '✅' : run.status === 'failed' ? '❌' : '⏳';
          const ver = run.version_after != null
            ? `${run.version_before}→${run.version_after}`
            : `${run.version_before}`;
          const started = run.started_at ? run.started_at.slice(0, 19) : '-';

          console.log(
            [
              icon.padEnd(5),
              (run.command_type || '-').padEnd(22),
              (run.command_summary || '-').slice(0, 30).padEnd(30),
              ver.padEnd(7),
              started.padEnd(20),
            ].join(' │ ')
          );
        }
        console.log('─'.repeat(90));
        console.log('');
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

function severityRank(sev: IssueSeverity): number {
  if (sev === 'error') return 3;
  if (sev === 'warning') return 2;
  return 1;
}

function parseSeverity(value: string | undefined): IssueSeverity {
  if (!value) return 'info';
  const v = value.toLowerCase();
  if (v === 'error' || v === 'warning' || v === 'info') return v;
  console.error('Error: --severity must be one of: error | warning | info');
  process.exit(1);
}

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

function parseOperatingHoursFromNotes(notes: string | null | undefined): string | undefined {
  if (!notes) return undefined;
  const match = notes.match(/(?:^|\\s)Hours:\\s*([^|\\n]+)/i);
  if (!match) return undefined;
  const v = match[1].trim();
  return v || undefined;
}

function inferDurationMin(activity: Record<string, unknown>): number {
  const duration = activity.duration_min as number | null | undefined;
  if (typeof duration === 'number' && Number.isFinite(duration) && duration > 0) return duration;

  const start = activity.start_time as string | undefined;
  const end = activity.end_time as string | undefined;
  if (start && end) {
    const parse = (t: string): number | null => {
      const m = t.match(/^(\d{1,2}):(\d{2})$/);
      if (!m) return null;
      const hh = parseInt(m[1], 10);
      const mm = parseInt(m[2], 10);
      if (!Number.isFinite(hh) || !Number.isFinite(mm)) return null;
      return hh * 60 + mm;
    };
    const s = parse(start);
    const e = parse(end);
    if (s !== null && e !== null && e > s) return e - s;
  }

  return 60;
}

function buildDaySummaries(days: Array<Record<string, unknown>>): DaySummary[] {
  const summaries: DaySummary[] = [];
  for (const day of days) {
    const dayNumber = day.day_number as number;
    const date = (day.date as string) || '';
    const theme = (day.theme as string | null) || (day.day_type as string) || '';

    const activities: ResolvedActivity[] = [];
    const areas = new Set<string>();
    let totalDurationMin = 0;
    let fixedTimeCount = 0;
    let pendingBookings = 0;

    for (const sessionName of ['morning', 'noon', 'afternoon', 'evening'] as const) {
      const session = day[sessionName] as Record<string, unknown> | undefined;
      const acts = (session?.activities as Array<unknown> | undefined) ?? [];
      for (const act of acts) {
        if (typeof act === 'string') {
          const durationMin = 60;
          activities.push({
            id: `legacy_${dayNumber}_${sessionName}_${activities.length}`,
            title: act,
            day: dayNumber,
            session: sessionName,
            durationMin,
            isFixedTime: false,
            bookingRequired: false,
          });
          totalDurationMin += durationMin;
          continue;
        }

        const a = act as Record<string, unknown>;
        const id = (a.id as string) || `activity_${dayNumber}_${sessionName}_${activities.length}`;
        const title = (a.title as string) || '';
        const area = (a.area as string) || undefined;
        if (area) areas.add(area);

        const durationMin = inferDurationMin(a);
        totalDurationMin += durationMin;

        const isFixedTime = Boolean(a.is_fixed_time);
        if (isFixedTime) fixedTimeCount++;

        const bookingRequired = Boolean(a.booking_required);
        const bookingStatus = a.booking_status as string | undefined;
        const bookByDate = a.book_by as string | undefined;
        if (bookingRequired && bookingStatus !== 'booked') pendingBookings++;

        activities.push({
          id,
          title,
          day: dayNumber,
          session: sessionName,
          startTime: a.start_time as string | undefined,
          endTime: a.end_time as string | undefined,
          durationMin,
          isFixedTime,
          area,
          bookingRequired,
          bookingStatus,
          bookByDate,
          operatingHours: parseOperatingHoursFromNotes(a.notes as string | null | undefined),
        });
      }
    }

    summaries.push({
      dayNumber,
      date,
      theme,
      activities,
      areas: Array.from(areas),
      totalDurationMin,
      fixedTimeCount,
      pendingBookings,
    });
  }
  return summaries;
}

function printValidationResult(destination: string, result: { valid: boolean; summary: any; issues: any[] }, threshold: IssueSeverity): void {
  const status = result.valid ? '✅ VALID' : '❌ ISSUES FOUND';
  console.log(`\n🧪 validate-itinerary (${destination})`);
  console.log(`   Result: ${status}`);
  console.log(`   Showing: ${threshold}+`);
  console.log(`   Summary: ${result.summary.errors} error(s), ${result.summary.warnings} warning(s), ${result.summary.info} info`);

  if (result.issues.length === 0) {
    console.log('\n(no issues to show)\n');
    return;
  }

  console.log('\nIssues:');
  for (const i of result.issues) {
    const where = [
      typeof i.day === 'number' ? `Day ${i.day}` : null,
      i.session ? `${i.session}` : null,
    ].filter(Boolean).join(' ');
    const prefix = i.severity === 'error' ? '⛔' : i.severity === 'warning' ? '⚠️' : 'ℹ️';
    console.log(`  ${prefix} ${where ? `${where}: ` : ''}${i.message}`);
    if (i.suggestion) console.log(`     → ${i.suggestion}`);
  }
  console.log('');
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
    const icon = r.success ? '✅' : '❌';
    console.log(`  ${icon} ${r.provenance.sourceId}: ${r.provenance.offersFound} offer(s) in ${r.provenance.duration_ms}ms`);
    if (r.errors.length) console.log(`     Errors: ${r.errors.slice(0, 2).join(' | ')}`);
    if (r.warnings.length) console.log(`     Warnings: ${r.warnings.slice(0, 2).join(' | ')}`);
    for (const o of r.offers.slice(0, 5)) {
      const price = o.priceTotal ?? o.pricePerPerson;
      const priceLabel = price ? `${o.currency} ${price.toLocaleString()}` : '(no price)';
      console.log(`     - ${o.title} — ${priceLabel} — ${o.availability}`);
    }
    if (r.offers.length > 5) console.log(`     ... and ${r.offers.length - 5} more`);
  }
  console.log('');
}

function normalizeScrapeToOffer(scrape: any, pax: number, warnings: string[]): any {
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

// ─── view-prices: Package vs Separate Booking Comparison ───────────────────

interface DateRangeResult {
  depart_date: string;
  return_date: string;
  depart_day: string;
  return_day: string;
  outbound: {
    nonstop_cheapest_usd: number | null;
    nonstop_cheapest_airline: string | null;
    nonstop_cheapest_time: string | null;
    flights: Array<{
      airline: string;
      depart: string;
      arrive: string;
      duration: string;
      nonstop: boolean;
      price_per_person_usd: number;
      total_usd: number;
    }>;
  };
  inbound: {
    nonstop_cheapest_usd: number | null;
    nonstop_cheapest_airline: string | null;
    nonstop_cheapest_time: string | null;
    flights: Array<{
      airline: string;
      depart: string;
      arrive: string;
      duration: string;
      nonstop: boolean;
      price_per_person_usd: number;
      total_usd: number;
    }>;
  };
  combined_cheapest_usd: number | null;
  combined_cheapest_twd: number | null;
}

interface DateRangeData {
  scraped_at: string;
  params: {
    depart_start: string;
    depart_end: string;
    origin: string;
    dest: string;
    duration: number;
    pax: number;
  };
  results: DateRangeResult[];
}

interface ViewPricesOptions {
  hotelPerNight?: number;
  nights?: number;
  packagePrice?: number;
  pax: number;
  json: boolean;
}

function autoDetectHotelPrice(): number | null {
  const dataDir = path.join(process.cwd(), 'data');
  if (!fs.existsSync(dataDir)) return null;

  const bookingFiles = fs.readdirSync(dataDir)
    .filter(f => f.startsWith('booking-') && f.endsWith('.json'))
    .sort()
    .reverse(); // newest first by name

  for (const file of bookingFiles) {
    try {
      const content = fs.readFileSync(path.join(dataDir, file), 'utf-8');
      const data = JSON.parse(content);
      const extracted = data.extracted || {};
      if (extracted.hotels && Array.isArray(extracted.hotels) && extracted.hotels.length > 0) {
        // Find cheapest hotel per-night price
        let cheapest: number | null = null;
        for (const hotel of extracted.hotels) {
          const price = hotel.price_per_night || hotel.price;
          if (typeof price === 'number' && (cheapest === null || price < cheapest)) {
            cheapest = price;
          }
        }
        if (cheapest !== null) return cheapest;
      }
      // Fallback: look for price patterns in raw_text
      const rawText = String(data.raw_text || '');
      const priceMatches = rawText.match(/TWD\s+([\d,]+)/g);
      if (priceMatches && priceMatches.length > 0) {
        const prices = priceMatches
          .map(m => parseInt(m.replace(/TWD\s+/, '').replace(/,/g, ''), 10))
          .filter(p => p > 1000 && p < 50000)
          .sort((a, b) => a - b);
        if (prices.length > 0) return prices[0];
      }
    } catch {
      // skip
    }
  }
  return null;
}

function showPriceComparison(flightsPath: string, opts: ViewPricesOptions): void {
  const resolvedPath = path.isAbsolute(flightsPath) ? flightsPath : path.join(process.cwd(), flightsPath);
  if (!fs.existsSync(resolvedPath)) {
    console.error(`Error: Flight data file not found: ${resolvedPath}`);
    process.exit(1);
  }

  let data: DateRangeData;
  try {
    data = JSON.parse(fs.readFileSync(resolvedPath, 'utf-8'));
  } catch (e) {
    console.error(`Error: Failed to parse flight data: ${(e as Error).message}`);
    process.exit(1);
  }

  if (!data.results || data.results.length === 0) {
    console.error('Error: No flight results found in data file.');
    process.exit(1);
  }

  const duration = data.params.duration;
  const pax = opts.pax || data.params.pax || 2;
  const hotelNights = opts.nights ?? (duration - 1);
  const usdToTwd = 32;

  // Auto-detect or use provided hotel price
  let hotelPerNight = opts.hotelPerNight;
  let hotelSource = 'manual';
  if (hotelPerNight === undefined) {
    const detected = autoDetectHotelPrice();
    if (detected !== null) {
      hotelPerNight = detected;
      hotelSource = 'auto (booking-*.json)';
    }
  }

  // Build comparison rows
  const rows: Array<{
    departDate: string;
    departDay: string;
    returnDate: string;
    returnDay: string;
    outAirline: string;
    outTime: string;
    outPrice: number;
    inAirline: string;
    inTime: string;
    inPrice: number;
    flightTotal: number;
    hotelTotal: number | null;
    separateTotal: number | null;
    packagePrice: number | null;
    diff: number | null;
    leaveDays: number;
  }> = [];

  for (const r of data.results) {
    const outPrice = r.outbound.nonstop_cheapest_usd;
    const inPrice = r.inbound.nonstop_cheapest_usd;
    if (outPrice === null || inPrice === null) continue;

    const flightTotalTwd = Math.round((outPrice + inPrice) * usdToTwd);
    const hotelTotalTwd = hotelPerNight !== undefined ? hotelPerNight * hotelNights : null;
    const separateTotal = hotelTotalTwd !== null ? flightTotalTwd + hotelTotalTwd : null;
    const diff = separateTotal !== null && opts.packagePrice !== undefined
      ? opts.packagePrice - separateTotal
      : null;

    // Calculate leave days
    const leavePlan = calculateLeave({
      startDate: r.depart_date,
      endDate: r.return_date,
      market: 'tw',
    });

    rows.push({
      departDate: r.depart_date,
      departDay: r.depart_day,
      returnDate: r.return_date,
      returnDay: r.return_day,
      outAirline: r.outbound.nonstop_cheapest_airline || '?',
      outTime: r.outbound.nonstop_cheapest_time || '?',
      outPrice,
      inAirline: r.inbound.nonstop_cheapest_airline || '?',
      inTime: r.inbound.nonstop_cheapest_time || '?',
      inPrice,
      flightTotal: flightTotalTwd,
      hotelTotal: hotelTotalTwd,
      separateTotal,
      packagePrice: opts.packagePrice ?? null,
      diff,
      leaveDays: leavePlan.leaveDaysNeeded,
    });
  }

  if (rows.length === 0) {
    console.error('Error: No valid flight data with nonstop prices found.');
    process.exit(1);
  }

  // Sort by separate total (cheapest first)
  rows.sort((a, b) => (a.separateTotal ?? Infinity) - (b.separateTotal ?? Infinity));

  if (opts.json) {
    console.log(JSON.stringify({ params: data.params, pax, hotelPerNight, hotelNights, hotelSource, rows }, null, 2));
    return;
  }

  // Print header
  console.log(`\n╔══════════════════════════════════════════════════════════════════════════════════╗`);
  console.log(`║                    PACKAGE vs SEPARATE BOOKING COMPARISON                       ║`);
  console.log(`╚══════════════════════════════════════════════════════════════════════════════════╝`);
  console.log(`  Route: ${data.params.origin.toUpperCase()} → ${data.params.dest.toUpperCase()} | ${duration} days | ${pax} pax`);
  console.log(`  Flight data: ${path.basename(flightsPath)} (scraped: ${data.scraped_at.slice(0, 16)})`);
  if (hotelPerNight !== undefined) {
    console.log(`  Hotel: TWD ${hotelPerNight.toLocaleString()}/night × ${hotelNights} nights = TWD ${(hotelPerNight * hotelNights).toLocaleString()} (${hotelSource})`);
  }
  if (opts.packagePrice !== undefined) {
    console.log(`  Package baseline: TWD ${opts.packagePrice.toLocaleString()} (${pax} pax)`);
  }
  console.log('');

  // Print comparison table
  console.log('┌────────────┬────────────┬──────────────────────┬──────────────────────┬────────────┬────────────┬────────────┬────────┐');
  console.log('│ 出發日     │ 回程日     │ 去程 (直飛最便宜)    │ 回程 (直飛最便宜)    │ 機票合計   │ 飯店合計   │ 分開訂合計 │ 請假   │');
  console.log('├────────────┼────────────┼──────────────────────┼──────────────────────┼────────────┼────────────┼────────────┼────────┤');

  for (let i = 0; i < rows.length; i++) {
    const r = rows[i];
    const marker = i === 0 ? ' ★' : '';
    const departCol = `${r.departDate.slice(5)} (${r.departDay})`.padEnd(10);
    const returnCol = `${r.returnDate.slice(5)} (${r.returnDay})`.padEnd(10);

    const outStr = `US$${r.outPrice} ${r.outAirline.slice(0, 8)}`.padEnd(20);
    const inStr = `US$${r.inPrice} ${r.inAirline.slice(0, 8)}`.padEnd(20);
    const flightStr = `TWD ${r.flightTotal.toLocaleString()}`.padEnd(10);
    const hotelStr = r.hotelTotal !== null ? `TWD ${r.hotelTotal.toLocaleString()}`.padEnd(10) : '—'.padEnd(10);
    const totalStr = r.separateTotal !== null ? `TWD ${r.separateTotal.toLocaleString()}`.padEnd(10) : '—'.padEnd(10);
    const leaveStr = `${r.leaveDays}天${marker}`.padEnd(6);

    console.log(`│ ${departCol} │ ${returnCol} │ ${outStr} │ ${inStr} │ ${flightStr} │ ${hotelStr} │ ${totalStr} │ ${leaveStr} │`);
  }

  console.log('└────────────┴────────────┴──────────────────────┴──────────────────────┴────────────┴────────────┴────────────┴────────┘');

  // Package comparison
  if (opts.packagePrice !== undefined) {
    console.log('\n  Package vs Separate:');
    for (const r of rows) {
      if (r.separateTotal === null || r.diff === null) continue;
      const diffStr = r.diff > 0
        ? `套餐貴 TWD ${r.diff.toLocaleString()} (+${Math.round(r.diff / r.separateTotal * 100)}%)`
        : r.diff < 0
          ? `分開訂貴 TWD ${Math.abs(r.diff).toLocaleString()} (+${Math.round(Math.abs(r.diff) / opts.packagePrice * 100)}%)`
          : '價格相同';
      console.log(`    ${r.departDate.slice(5)} (${r.departDay}): ${diffStr}`);
    }
  }

  // Best value summary
  const best = rows[0];
  console.log(`\n  ★ 最便宜: ${best.departDate} (${best.departDay}) 出發`);
  console.log(`    去程: ${best.outAirline} ${best.outTime} — US$${best.outPrice}`);
  console.log(`    回程: ${best.inAirline} ${best.inTime} — US$${best.inPrice}`);
  if (best.separateTotal !== null) {
    console.log(`    分開訂合計: TWD ${best.separateTotal.toLocaleString()} (${pax}人)`);
  }
  console.log(`    請假: ${best.leaveDays}天`);

  // LCC baggage warning
  console.log('\n  ⚠️  LCC 不含託運行李，需另加購約 TWD 1,500-2,000/人 (來回)');
  console.log('');
}

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
    besttour: '喜鴻假期',
    liontravel: '雄獅旅遊',
    lifetour: '五福旅遊',
    settour: '東南旅遊',
    eztravel: '易遊網',
    tigerair: '台灣虎航',
  };

  // Determine type from URL or content
  let type: 'package' | 'group_tour' | 'fit' = 'package';
  if (url.includes('vacation.liontravel.com') || url.includes('自由配')) {
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

function inferSourceIdFromFilename(filename: string): string {
  if (filename.includes('besttour')) return 'besttour';
  if (filename.includes('liontravel')) return 'liontravel';
  if (filename.includes('lifetour')) return 'lifetour';
  if (filename.includes('settour')) return 'settour';
  if (filename.includes('eztravel')) return 'eztravel';
  if (filename.includes('tigerair')) return 'tigerair';
  return 'unknown';
}

function printOfferComparison(offers: CompareOffer[], region: string, filterDate: string | undefined, pax: number): void {
  console.log(`\n╔════════════════════════════════════════════════════════════════════════╗`);
  console.log(`║  PACKAGE COMPARISON: ${region.toUpperCase().padEnd(48)}║`);
  console.log(`╚════════════════════════════════════════════════════════════════════════╝`);
  console.log(`  Pax: ${pax} | Filter date: ${filterDate || '(all)'}`);
  console.log(`  Found ${offers.length} scraped file(s)\n`);

  if (offers.length === 0) return;

  // Print table header
  console.log('┌─────────────────┬─────────────────┬─────────────────┬────────────────────────────────┐');
  const totalHeader = `Total (${pax}pax)`.padEnd(15).slice(0, 15);
  console.log(`│ OTA             │ Price/person    │ ${totalHeader} │ Details                        │`);
  console.log('├─────────────────┼─────────────────┼─────────────────┼────────────────────────────────┤');

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

    console.log(`│ ${o.source_name.padEnd(15)} │ ${priceStr.padEnd(15)} │ ${totalStr.padEnd(15)} │ ${detailStr.padEnd(30)} │`);
  }

  console.log('└─────────────────┴─────────────────┴─────────────────┴────────────────────────────────┘');

  // Print staleness warning for old data
  const now = Date.now();
  const staleThresholdMs = FRESHNESS.STALE_THRESHOLD_MS;
  const staleOffers = offers.filter(o => {
    if (!o.scraped_at) return false;
    const scrapedTime = new Date(o.scraped_at).getTime();
    return now - scrapedTime > staleThresholdMs;
  });

  if (staleOffers.length > 0) {
    console.log(`\n⚠️  ${staleOffers.length} offer(s) have stale data (>24h old). Consider re-scraping.`);
  }

  // Print best value recommendation
  const bestOffer = offers[0];
  if (bestOffer && bestOffer.price_per_person !== null) {
    console.log(`\n💡 Best value: ${bestOffer.source_name} at ${bestOffer.currency} ${bestOffer.price_per_person.toLocaleString()}/person`);
    if (bestOffer.hotel) console.log(`   Hotel: ${bestOffer.hotel}`);
    if (bestOffer.airline) console.log(`   Airline: ${bestOffer.airline}`);
  }

  console.log('');
}

main();

/**
 * Resolve destination reference file path using config loader.
 * Falls back to config-based resolution instead of hardcoded Tokyo check.
 */
function resolveDestinationRefPath(destinationSlug: string): string | null {
  return configResolveDestinationRefPath(destinationSlug);
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

function getSessionOrderForDayType(dayType: string): Array<'morning' | 'noon' | 'afternoon' | 'evening'> {
  if (dayType === 'arrival') return ['afternoon', 'evening'];
  if (dayType === 'departure') return ['morning', 'noon'];
  return ['morning', 'noon', 'afternoon', 'evening'];
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
