import type { CommandHandler, CliContext } from '../shared/types';
import { registerCommand } from './registry';
import { formatDate } from '../shared/output';

function showTransport(ctx: CliContext): void {
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

const transportCommand: CommandHandler = {
  names: ['transport'],
  description: 'Show transport summary (airport + daily transit)',
  usage: 'transport [--dest slug]',
  async execute(ctx: CliContext): Promise<void> {
    showTransport(ctx);
  },
};

registerCommand(transportCommand);
