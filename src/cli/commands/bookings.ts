import type { CommandHandler, CliContext } from '../shared/types';
import { registerCommand } from './registry';

function showBookings(ctx: CliContext): void {
  const { sm, args } = ctx;
  const destOpt = args.optionValue('--dest');
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

const bookingsCommand: CommandHandler = {
  names: ['bookings'],
  description: 'Show pending bookings only',
  usage: 'bookings [--dest slug]',
  async execute(ctx: CliContext): Promise<void> {
    showBookings(ctx);
  },
};

registerCommand(bookingsCommand);
