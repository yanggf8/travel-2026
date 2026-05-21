import type { CommandHandler, CliContext } from '../shared/types';
import { registerCommand } from './registry';
import { formatDate } from '../shared/output';

const scaffoldItineraryCommand: CommandHandler = {
  names: ['scaffold-itinerary'],
  description: 'Create day skeletons for P5 itinerary based on date anchor.',
  usage: 'scaffold-itinerary [--dest slug] [--force]',
  async execute(ctx: CliContext): Promise<void> {
    const { sm, args, dryRun } = ctx;
    const destination = sm.resolveDestination(args.optionValue('--dest'));
    const force = args.hasFlag('--force');
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
  },
};

registerCommand(scaffoldItineraryCommand);
