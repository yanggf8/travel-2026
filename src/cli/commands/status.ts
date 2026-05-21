import type { CommandHandler, CliContext } from '../shared/types';
import { registerCommand } from './registry';
import { formatDate } from '../shared/output';
import type { StateManager } from '../../state/state-manager';
import type { ProcessId } from '../../state/types';

export function showStatus(sm: StateManager, opts?: { full?: boolean }): void {
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

const statusCommand: CommandHandler = {
  names: ['status'],
  description: 'Show current plan status summary',
  usage: 'status [--full]',
  async execute(ctx: CliContext): Promise<void> {
    showStatus(ctx.sm, { full: ctx.args.hasFlag('--full') });
  },
};

registerCommand(statusCommand);
