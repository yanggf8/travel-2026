import type { CommandHandler, CliContext } from '../shared/types';
import { registerCommand } from './registry';
import type { FlightLegInput } from '../../state/repository';

const setFlightCommand: CommandHandler = {
  names: ['set-flight'],
  description: 'Manually set/update a flight leg.',
  usage: 'set-flight <outbound|return> [--dest slug] [--flight SL396] [--airline "Thai Lion Air"] ...',
  async execute(ctx: CliContext): Promise<void> {
    const { sm, args, dryRun } = ctx;
    const [, direction] = args.cleanArgs;
    const destOpt = args.optionValue('--dest');

    if (!direction || !['outbound', 'return'].includes(direction)) {
      console.error('Error: set-flight requires <outbound|return>');
      console.error('Example: set-flight outbound --dest kyoto_2026 --flight SL396 --airline "Thai Lion Air" --from TPE --dep 09:00 --to KIX --arr 12:30');
      process.exit(1);
    }

    const destination = sm.resolveDestination(destOpt);
    const input: FlightLegInput = {};
    const flightOpt = args.optionValue('--flight');
    const airlineOpt = args.optionValue('--airline');
    const airlineCodeOpt = args.optionValue('--airline-code');
    const fromOpt = args.optionValue('--from');
    const depTerminalOpt = args.optionValue('--dep-terminal');
    const depOpt = args.optionValue('--dep');
    const toOpt = args.optionValue('--to');
    const arrTerminalOpt = args.optionValue('--arr-terminal');
    const arrOpt = args.optionValue('--arr');
    const dateOpt = args.optionValue('--date');
    const bookedDateOpt = args.optionValue('--booked-date');

    if (flightOpt) input.flightNumber = flightOpt;
    if (airlineOpt) input.airline = airlineOpt;
    if (airlineCodeOpt) input.airlineCode = airlineCodeOpt;
    if (fromOpt) input.departureCode = fromOpt;
    if (depTerminalOpt) input.departureTerminal = depTerminalOpt;
    if (depOpt) input.departureTime = depOpt;
    if (toOpt) input.arrivalCode = toOpt;
    if (arrTerminalOpt) input.arrivalTerminal = arrTerminalOpt;
    if (arrOpt) input.arrivalTime = arrOpt;
    if (dateOpt) input.date = dateOpt;
    if (bookedDateOpt) input.bookedDate = bookedDateOpt;

    console.log(`\n✈️  Setting flight leg:`);
    console.log(`   Destination: ${destination}`);
    console.log(`   Direction: ${direction}`);
    if (input.flightNumber) console.log(`   Flight: ${input.flightNumber}`);
    if (input.airline) console.log(`   Airline: ${input.airline} (${input.airlineCode ?? ''})`);
    if (input.departureCode) console.log(`   From: ${input.departureCode}${input.departureTerminal ? ' T' + input.departureTerminal : ''} ${input.departureTime ?? ''}`);
    if (input.arrivalCode) console.log(`   To: ${input.arrivalCode}${input.arrivalTerminal ? ' T' + input.arrivalTerminal : ''} ${input.arrivalTime ?? ''}`);
    if (input.date) console.log(`   Date: ${input.date}`);

    if (!dryRun) {
      sm.setFlightLeg(destination, direction as 'outbound' | 'return', input);
      await sm.saveWithTracking('set-flight', `${destination} ${direction} ${input.flightNumber ?? ''}`);
      console.log('✅ Flight leg updated');
    } else {
      console.log('🔸 DRY RUN - no changes saved');
    }
  },
};

registerCommand(setFlightCommand);
