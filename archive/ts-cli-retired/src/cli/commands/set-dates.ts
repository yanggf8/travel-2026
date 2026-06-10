import type { CommandHandler, CliContext } from '../shared/types';
import { registerCommand } from './registry';
import { formatDate } from '../shared/output';
import { showStatus } from './status';
import { validateDateRange } from '../../types/validation';

const setDatesCommand: CommandHandler = {
  names: ['set-dates'],
  description: 'Set travel dates. Triggers cascade to invalidate dependent processes.',
  usage: 'set-dates <start> <end> [reason]',
  async execute(ctx: CliContext): Promise<void> {
    const { sm, args, dryRun, verbose } = ctx;
    const [, startDate, endDate, ...reasonParts] = args.cleanArgs;
    if (!startDate || !endDate) {
      console.error('Error: set-dates requires <start> and <end> dates');
      console.error('Example: set-dates 2026-02-13 2026-02-17 "Agent offered Feb 13"');
      process.exit(1);
    }

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
  },
};

registerCommand(setDatesCommand);
