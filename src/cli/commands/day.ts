import type { CommandHandler, CliContext } from '../shared/types';
import { registerCommand } from './registry';
import { requireDayArg } from '../shared/validation';

// ── set-day-theme ──────────────────────────────────────────────────

const setDayThemeCommand: CommandHandler = {
  names: ['set-day-theme'],
  description: 'Set the theme (and optional Chinese title) for a day.',
  usage: 'set-day-theme <day> [theme] [--zh "<chinese_title>"] [--dest <slug>]',
  async execute(ctx: CliContext): Promise<void> {
    const { sm, args, dryRun } = ctx;
    const [, dayStr, themeArg] = args.cleanArgs;
    const destOpt = args.optionValue('--dest');

    if (!dayStr) {
      console.error('Error: set-day-theme requires <day> and --zh "<title>"');
      console.error('Example: set-day-theme 1 --zh "抵達京都"');
      console.error('Example: set-day-theme 2 "Kinkaku-ji full day" --zh "金閣寺・伏見稲荷"');
      process.exit(1);
    }

    const dayNumber = requireDayArg(dayStr);
    const themeZhOpt = args.optionValue('--zh');

    // themeArg is optional (update only ZH if omitted)
    const destination = sm.resolveDestination(destOpt);

    if (!dryRun) {
      sm.setDayTheme(destination, dayNumber, themeArg ?? null, themeZhOpt ?? undefined);
      await sm.saveWithTracking('set-day-theme', `D${dayNumber}`);
      console.log('✅ Day theme updated');
    } else {
      console.log('🔸 DRY RUN - no changes saved');
    }
  },
};

registerCommand(setDayThemeCommand);

// ── swap-days ──────────────────────────────────────────────────────

const swapDaysCommand: CommandHandler = {
  names: ['swap-days'],
  description: 'Swap two days in the itinerary.',
  usage: 'swap-days <dayA> <dayB> [--dest <slug>]',
  async execute(ctx: CliContext): Promise<void> {
    const { sm, args, dryRun } = ctx;
    const [, dayAStr, dayBStr] = args.cleanArgs;
    const destOpt = args.optionValue('--dest');

    if (!dayAStr || !dayBStr) {
      console.error('Error: swap-days requires <dayA> <dayB>');
      console.error('Example: swap-days 2 3');
      process.exit(1);
    }

    const dayA = requireDayArg(dayAStr, '<dayA>');
    const dayB = requireDayArg(dayBStr, '<dayB>');

    if (dayA === dayB) {
      console.error('Error: dayA and dayB must be different');
      process.exit(1);
    }

    const destination = sm.resolveDestination(destOpt);
    console.log(`\n🔄 Swapping days:`);
    console.log(`   Destination: ${destination}`);
    console.log(`   Day ${dayA} ↔ Day ${dayB}`);

    if (!dryRun) {
      sm.swapDays(destination, dayA, dayB);
      await sm.saveWithTracking('swap-days', `D${dayA} ↔ D${dayB}`);
      console.log('✅ Days swapped successfully');
    } else {
      console.log('🔸 DRY RUN - no changes saved');
    }
  },
};

registerCommand(swapDaysCommand);
