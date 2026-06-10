import type { CommandHandler, CliContext } from '../shared/types';
import { registerCommand } from './registry';
import type { ProcessId } from '../../state/types';

const markBookedCommand: CommandHandler = {
  names: ['mark-booked'],
  description: 'Mark package, flight, and hotel as booked (selected/populated → booking → booked).',
  usage: 'mark-booked [--dest slug]',
  async execute(ctx: CliContext): Promise<void> {
    const { sm, args, dryRun } = ctx;
    const destination = sm.resolveDestination(args.optionValue('--dest'));
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
  },
};

registerCommand(markBookedCommand);
