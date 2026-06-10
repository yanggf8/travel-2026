import type { CommandHandler, CliContext } from '../shared/types';
import { registerCommand } from './registry';
import { formatDate } from '../shared/output';
import { showStatus } from './status';

const updateOfferCommand: CommandHandler = {
  names: ['update-offer'],
  description: 'Update offer availability for a specific date.',
  usage: 'update-offer <offer-id> <date> <availability> [price] [seats] [source]',
  async execute(ctx: CliContext): Promise<void> {
    const { sm, args, dryRun } = ctx;
    const [, offerId, date, availability, priceStr, seatsStr, source] = args.cleanArgs;
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
  },
};

const selectOfferCommand: CommandHandler = {
  names: ['select-offer'],
  description: 'Select an offer for booking. Populates P3/P4 from offer by default.',
  usage: 'select-offer <offer-id> <date> [--no-populate]',
  async execute(ctx: CliContext): Promise<void> {
    const { sm, args, dryRun, verbose } = ctx;
    const [, offerId, date] = args.cleanArgs;
    const destOpt = args.optionValue('--dest');
    const noPopulate = args.hasFlag('--no-populate');

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
    } else {
      console.log('🔸 DRY RUN - no changes saved');
    }

    if (verbose) showStatus(sm);
  },
};

registerCommand(updateOfferCommand);
registerCommand(selectOfferCommand);
