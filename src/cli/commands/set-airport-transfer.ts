import type { CommandHandler, CliContext } from '../shared/types';
import { registerCommand } from './registry';
import { showStatus } from './status';
import { parseTransferSpec } from '../shared/transfer-parsing';

const setAirportTransferCommand: CommandHandler = {
  names: ['set-airport-transfer'],
  description: 'Set airport transfer plan (selected + candidates) for arrival/departure.',
  usage: 'set-airport-transfer <arrival|departure> <planned|booked> --selected "<title|route|...>" [--candidate "<...>"]...',
  async execute(ctx: CliContext): Promise<void> {
    const { sm, args, dryRun, verbose } = ctx;
    const [, direction, status] = args.cleanArgs;
    const destOpt = args.optionValue('--dest');
    const selectedOpt = args.optionValue('--selected');
    const candidateOpts = args.optionValues('--candidate');
    const full = args.hasFlag('--full');

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
  },
};

registerCommand(setAirportTransferCommand);
