import type { CommandHandler, CliContext } from '../shared/types';
import { registerCommand } from './registry';
import type { HotelInput } from '../../state/repository';

const setHotelCommand: CommandHandler = {
  names: ['set-hotel'],
  description: 'Manually set/update hotel info.',
  usage: 'set-hotel [--dest slug] [--name "Hotel Name"] [--check-in YYYY-MM-DD] [--access "route"]',
  async execute(ctx: CliContext): Promise<void> {
    const { sm, args, dryRun } = ctx;
    const destOpt = args.optionValue('--dest');
    const hotelNameOpt = args.optionValue('--name');
    const accessOpt = args.optionValue('--access');
    const checkInOpt = args.optionValue('--check-in');
    const noteOpt = args.optionValue('--note');

    const destination = sm.resolveDestination(destOpt);
    const input: HotelInput = {};
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
  },
};

registerCommand(setHotelCommand);
