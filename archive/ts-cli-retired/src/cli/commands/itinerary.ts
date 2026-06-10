import type { CommandHandler, CliContext } from '../shared/types';
import { registerCommand } from './registry';
import { formatDate } from '../shared/output';

function showItinerary(ctx: CliContext): void {
  const { sm, args } = ctx;
  const destOpt = args.optionValue('--dest');
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
      const icon = (weather.weather_code ?? 0) >= 71 && (weather.weather_code ?? 0) <= 77 ? '❄️' :
                   (weather.precipitation_pct ?? 0) > 50 ? '🌧️' :
                   (weather.weather_code ?? 0) >= 2 ? '⛅' : '☀️';
      const srcDate = weather.sourced_at ? weather.sourced_at.split('T')[0] : '';
      console.log(`${icon} ${weather.weather_label} | ${weather.temp_low_c}–${weather.temp_high_c}°C | Rain: ${weather.precipitation_pct}%  (${weather.source_id || 'unknown'}, ${srcDate})`);
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

const itineraryCommand: CommandHandler = {
  names: ['itinerary'],
  description: 'Show daily itinerary with transport details',
  usage: 'itinerary [--dest slug]',
  async execute(ctx: CliContext): Promise<void> {
    showItinerary(ctx);
  },
};

registerCommand(itineraryCommand);
