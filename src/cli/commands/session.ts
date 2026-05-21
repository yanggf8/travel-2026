import type { CommandHandler, CliContext } from '../shared/types';
import { registerCommand } from './registry';
import { showStatus } from './status';
import { requireDayArg, requireSessionArg } from '../shared/validation';
import { validateTime } from '../../types/validation';
import type { SessionType } from '../../state/types';

// ── set-tod-focus / set-session-focus ──────────────────────────────

const setTodFocusCommand: CommandHandler = {
  names: ['set-tod-focus', 'set-session-focus'],
  description: 'Set the focus text for a day/session.',
  usage: 'set-tod-focus <day> <session> "<focus_text>" [--plan-id <id>]',
  async execute(ctx: CliContext): Promise<void> {
    const { sm, args, dryRun } = ctx;
    const [, dayStr, sessionArg, focusText] = args.cleanArgs;
    const destOpt = args.optionValue('--dest');

    if (!dayStr || !sessionArg) {
      console.error('Usage: set-tod-focus <day> <session> "<focus_text>"');
      console.error('Example: set-tod-focus 2 morning "Kitano Tenmangu → Kinkaku-ji"');
      process.exit(1);
    }
    const dayNum3 = requireDayArg(dayStr);
    requireSessionArg(sessionArg);
    const dest3 = sm.resolveDestination(destOpt);
    if (!dryRun) {
      await sm.dispatch({
        type: 'set_session_focus',
        destination: dest3,
        dayNumber: dayNum3,
        session: sessionArg as SessionType,
        focus: focusText ?? null,
      });
      await sm.saveWithTracking('set-tod-focus', `D${dayNum3}/${sessionArg}`);
      console.log(`✅ Focus set for D${dayNum3} ${sessionArg}: "${focusText ?? '(cleared)'}"`);
    } else {
      console.log(`🔸 DRY RUN — would set D${dayNum3} ${sessionArg} focus: "${focusText}"`);
    }
  },
};

registerCommand(setTodFocusCommand);

// ── set-tod-time-range / set-session-time-range ───────────────────

const setTodTimeRangeCommand: CommandHandler = {
  names: ['set-tod-time-range', 'set-session-time-range'],
  description: 'Set start and end time for a session.',
  usage: 'set-session-time-range <day> <session> --start HH:MM --end HH:MM [--dest <slug>]',
  async execute(ctx: CliContext): Promise<void> {
    const { sm, args, dryRun, verbose } = ctx;
    const [, dayStr, session] = args.cleanArgs;
    const destOpt = args.optionValue('--dest');
    const startOpt = args.optionValue('--start');
    const endOpt = args.optionValue('--end');
    const full = args.hasFlag('--full');

    if (!dayStr || !session) {
      console.error('Error: set-session-time-range requires <day> <session>');
      console.error('Example: set-session-time-range 5 afternoon --start 11:00 --end 14:45');
      process.exit(1);
    }

    const dayNumber = requireDayArg(dayStr);
    requireSessionArg(session);

    if (!startOpt || !endOpt) {
      console.error('Error: set-session-time-range requires --start HH:MM and --end HH:MM');
      process.exit(1);
    }

    // Validate time formats
    const startTimeResult = validateTime(startOpt, '--start');
    if (!startTimeResult.ok) {
      console.error(`Error: ${startTimeResult.error}`);
      process.exit(1);
    }
    const endTimeResult = validateTime(endOpt, '--end');
    if (!endTimeResult.ok) {
      console.error(`Error: ${endTimeResult.error}`);
      process.exit(1);
    }

    const destination = sm.resolveDestination(destOpt);
    console.log(`\n🕒 Setting session time range:`);
    console.log(`   Destination: ${destination}`);
    console.log(`   Day ${dayNumber} ${session}: ${startTimeResult.value} → ${endTimeResult.value}`);

    if (!dryRun) {
      sm.setSessionTimeRange(destination, dayNumber, session as any, startTimeResult.value, endTimeResult.value);
      await sm.saveWithTracking('set-session-time-range', `D${dayNumber} ${session}`);
      console.log('✅ Session time range updated');
    } else {
      console.log('🔸 DRY RUN - no changes saved');
    }

    if (verbose) showStatus(sm, { full });
  },
};

registerCommand(setTodTimeRangeCommand);

// ── set-tod-zh / set-session-zh ────────────────────────────────────

const setTodZhCommand: CommandHandler = {
  names: ['set-tod-zh', 'set-session-zh'],
  description: 'Set Chinese content (focus, transit notes, activities, meals) for a session.',
  usage: 'set-session-zh <day> <session> [--zh "..."] [--transit-zh "..."] [--activities-zh-json \'[...]\'] [--meals-zh-json \'[...]\'] [--plan-id <id>]',
  async execute(ctx: CliContext): Promise<void> {
    const { sm, args, dryRun } = ctx;
    const [, dayStr, sessionArg] = args.cleanArgs;
    const destOpt = args.optionValue('--dest');

    if (!dayStr || !sessionArg) {
      console.error('Error: set-session-zh requires <day> <session>');
      console.error('  --zh "Chinese focus text"');
      console.error('  --transit-zh "Chinese transit notes"');
      console.error('  --activities-zh-json \'["act1","act2"]\'');
      console.error('  --meals-zh-json \'["lunch: 拉麵","dinner: 壽司"]\'');
      console.error('Example: set-session-zh 2 morning --zh "金閣寺・可選龍安寺" --transit-zh "地鐵→北大路→公車→金閣寺道"');
      process.exit(1);
    }

    const sessionDay = requireDayArg(dayStr);
    requireSessionArg(sessionArg);
    const sessionType = sessionArg as 'morning' | 'noon' | 'afternoon' | 'evening';

    const focusZhOpt = args.optionValue('--zh') ?? undefined;
    const transitZhOpt = args.optionValue('--transit-zh') ?? undefined;
    const activitiesZhJsonOpt = args.optionValue('--activities-zh-json') ?? undefined;
    const mealsZhJsonOpt = args.optionValue('--meals-zh-json') ?? undefined;
    let activitiesZh: string[] | undefined;
    if (activitiesZhJsonOpt) {
      try {
        activitiesZh = JSON.parse(activitiesZhJsonOpt);
      } catch {
        console.error('Error: --activities-zh-json must be a valid JSON array of strings');
        process.exit(1);
      }
    }
    let mealsZh: string[] | undefined;
    if (mealsZhJsonOpt) {
      try {
        mealsZh = JSON.parse(mealsZhJsonOpt);
      } catch {
        console.error('Error: --meals-zh-json must be a valid JSON array of strings');
        process.exit(1);
      }
    }

    const destination = sm.resolveDestination(destOpt);
    console.log(`\n🌏 Setting session ZH content:`);
    console.log(`   Destination: ${destination}, Day ${sessionDay}, ${sessionType}`);
    if (focusZhOpt) console.log(`   focus_zh: "${focusZhOpt}"`);
    if (transitZhOpt) console.log(`   transit_notes_zh: "${transitZhOpt}"`);
    if (activitiesZh) console.log(`   activities_zh: ${activitiesZh.length} items`);
    if (mealsZh) console.log(`   meals_zh: ${mealsZh.length} items`);

    if (!dryRun) {
      sm.setSessionZhContent(destination, sessionDay, sessionType, focusZhOpt, transitZhOpt, activitiesZh, mealsZh);
      await sm.saveWithTracking('set-session-zh', `D${sessionDay}/${sessionType}`);
      console.log('✅ Session ZH content updated');
    } else {
      console.log('🔸 DRY RUN - no changes saved');
    }
  },
};

registerCommand(setTodZhCommand);
