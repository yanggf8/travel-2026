import type { CommandHandler, CliContext } from '../shared/types';
import { registerCommand } from './registry';
import { showStatus } from './status';
import { requireDayArg, requireSessionArg } from '../shared/validation';
import { validateTime, validateIsoDate } from '../../types/validation';
import type { SessionType } from '../../state/types';

// ── delete-activity / remove-activity ──────────────────────────────

const deleteActivityCommand: CommandHandler = {
  names: ['delete-activity', 'remove-activity'],
  description: 'Delete an activity from a day/session.',
  usage: 'delete-activity <day> <session> <activity_id_or_title> [--dest <slug>]',
  async execute(ctx: CliContext): Promise<void> {
    const { sm, args, dryRun } = ctx;
    const [, dayStr, sessionArg, activityArg] = args.cleanArgs;
    const destOpt = args.optionValue('--dest');

    if (!dayStr || !sessionArg || !activityArg) {
      console.error('Usage: delete-activity <day> <session> <activity_id_or_title>');
      console.error('Example: delete-activity 2 morning "teamLab Borderless"');
      process.exit(1);
    }
    const dayNum2 = requireDayArg(dayStr);
    requireSessionArg(sessionArg);
    const dest2 = sm.resolveDestination(destOpt);
    // Resolve activity within the specific day+session to avoid title-collision false rejects
    const day2 = sm.getDay(dest2, dayNum2);
    if (!day2) {
      console.error(`Error: Day ${dayNum2} not found in ${dest2}`);
      process.exit(1);
    }
    const sessionObj2 = day2[sessionArg] as { activities?: Array<string | Record<string, unknown>> } | undefined;
    const acts2 = sessionObj2?.activities ?? [];
    const searchLower2 = activityArg.toLowerCase();
    const matchAct2 = acts2.find((a: unknown) => {
      if (typeof a === 'string') return a.toLowerCase().includes(searchLower2);
      const r = a as Record<string, unknown>;
      return r.id === activityArg || (typeof r.title === 'string' && r.title.toLowerCase().includes(searchLower2));
    });
    if (!matchAct2) {
      console.error(`Error: Activity "${activityArg}" not found in Day ${dayNum2} ${sessionArg}`);
      const actList = acts2.map((a: unknown) => typeof a === 'string' ? `  "${a}"` : `  [${(a as Record<string,unknown>).id}] ${(a as Record<string,unknown>).title}`);
      if (actList.length) console.error('Activities in this session:\n' + actList.join('\n'));
      process.exit(1);
    }
    const matchTitle = typeof matchAct2 === 'string' ? matchAct2 : (matchAct2 as Record<string, unknown>).title as string;
    const matchId = typeof matchAct2 === 'string' ? matchAct2 : (matchAct2 as Record<string, unknown>).id as string;
    console.log(`\n🗑️  Deleting activity:`);
    console.log(`   Day ${dayNum2} ${sessionArg}: "${matchTitle}"`);
    if (!dryRun) {
      await sm.dispatch({
        type: 'remove_activity',
        destination: dest2,
        dayNumber: dayNum2,
        session: sessionArg as SessionType,
        activityId: matchId,
      });
      await sm.saveWithTracking('delete-activity', `D${dayNum2}/${sessionArg}/${matchTitle}`);
      console.log('✅ Activity deleted');
    } else {
      console.log('🔸 DRY RUN - no changes saved');
    }
  },
};

registerCommand(deleteActivityCommand);

// ── set-activity-time ──────────────────────────────────────────────

const setActivityTimeCommand: CommandHandler = {
  names: ['set-activity-time'],
  description: 'Set start/end time and fixed flag for an activity.',
  usage: 'set-activity-time <day> <session> <activity> [--start HH:MM] [--end HH:MM] [--fixed true|false] [--dest <slug>]',
  async execute(ctx: CliContext): Promise<void> {
    const { sm, args, dryRun, verbose } = ctx;
    const [, dayStr, session, activity] = args.cleanArgs;
    const destOpt = args.optionValue('--dest');
    const startOpt = args.optionValue('--start');
    const endOpt = args.optionValue('--end');
    const fixedOpt = args.optionValue('--fixed');
    const full = args.hasFlag('--full');

    if (!dayStr || !session || !activity) {
      console.error('Error: set-activity-time requires <day> <session> <activity>');
      console.error('Example: set-activity-time 5 afternoon "Hotel checkout" --start 11:00 --fixed true');
      process.exit(1);
    }

    const dayNumber = requireDayArg(dayStr);
    requireSessionArg(session);

    const parseFixed = (value: string | undefined): boolean | undefined => {
      if (value === undefined) return undefined;
      const v = value.toLowerCase();
      if (['true', '1', 'yes', 'y'].includes(v)) return true;
      if (['false', '0', 'no', 'n'].includes(v)) return false;
      throw new Error(`Invalid --fixed value: ${value} (use true|false)`);
    };

    let isFixed: boolean | undefined;
    try {
      isFixed = parseFixed(fixedOpt);
    } catch (e) {
      console.error(`Error: ${(e as Error).message}`);
      process.exit(1);
    }

    if (startOpt === undefined && endOpt === undefined && isFixed === undefined) {
      console.error('Error: set-activity-time requires at least one of: --start, --end, --fixed');
      process.exit(1);
    }

    // Validate time formats
    let validatedStart: string | undefined;
    let validatedEnd: string | undefined;
    if (startOpt) {
      const startResult = validateTime(startOpt, '--start');
      if (!startResult.ok) {
        console.error(`Error: ${startResult.error}`);
        process.exit(1);
      }
      validatedStart = startResult.value;
    }
    if (endOpt) {
      const endResult = validateTime(endOpt, '--end');
      if (!endResult.ok) {
        console.error(`Error: ${endResult.error}`);
        process.exit(1);
      }
      validatedEnd = endResult.value;
    }

    const destination = sm.resolveDestination(destOpt);

    console.log(`\n⏱️  Setting activity time:`);
    console.log(`   Destination: ${destination}`);
    console.log(`   Day ${dayNumber} ${session}: "${activity}"`);
    if (validatedStart) console.log(`   Start: ${validatedStart}`);
    if (validatedEnd) console.log(`   End: ${validatedEnd}`);
    if (isFixed !== undefined) console.log(`   Fixed: ${isFixed}`);

    if (!dryRun) {
      sm.setActivityTime(destination, dayNumber, session as any, activity, {
        start_time: validatedStart,
        end_time: validatedEnd,
        is_fixed_time: isFixed,
      });
      await sm.saveWithTracking('set-activity-time', `D${dayNumber} ${session} ${activity}`);
      console.log('✅ Activity time updated');
    } else {
      console.log('🔸 DRY RUN - no changes saved');
    }

    if (verbose) showStatus(sm, { full });
  },
};

registerCommand(setActivityTimeCommand);

// ── set-activity-booking ───────────────────────────────────────────

const setActivityBookingCommand: CommandHandler = {
  names: ['set-activity-booking'],
  description: 'Set booking status for an activity.',
  usage: 'set-activity-booking <day> <session> <activity> <status> [--ref "..."] [--book-by YYYY-MM-DD] [--dest <slug>]',
  async execute(ctx: CliContext): Promise<void> {
    const { sm, args, dryRun } = ctx;
    const [, dayStr, session, activity, status] = args.cleanArgs;
    const destOpt = args.optionValue('--dest');
    const refOpt = args.optionValue('--ref');
    const bookByOpt = args.optionValue('--book-by');

    if (!dayStr || !session || !activity || !status) {
      console.error('Error: set-activity-booking requires <day> <session> <activity> <status>');
      console.error('Example: set-activity-booking 3 morning "teamLab Borderless" booked --ref "TLB-12345"');
      process.exit(1);
    }

    const dayNumber = requireDayArg(dayStr);
    requireSessionArg(session);

    const validStatuses = ['not_required', 'pending', 'booked', 'waitlist'];
    if (!validStatuses.includes(status)) {
      console.error('Error: <status> must be one of: not_required | pending | booked | waitlist');
      process.exit(1);
    }

    // Validate book-by date if provided
    let validatedBookBy: string | undefined;
    if (bookByOpt) {
      const bookByResult = validateIsoDate(bookByOpt, '--book-by');
      if (!bookByResult.ok) {
        console.error(`Error: ${bookByResult.error}`);
        process.exit(1);
      }
      validatedBookBy = bookByResult.value;
    }

    const destination = sm.resolveDestination(destOpt);

    console.log(`\n🎫 Setting activity booking status:`);
    console.log(`   Destination: ${destination}`);
    console.log(`   Day ${dayNumber} ${session}: "${activity}"`);
    console.log(`   Status: ${status}`);
    if (refOpt) console.log(`   Reference: ${refOpt}`);
    if (validatedBookBy) console.log(`   Book by: ${validatedBookBy}`);

    if (!dryRun) {
      sm.setActivityBookingStatus(
        destination,
        dayNumber,
        session as 'morning' | 'noon' | 'afternoon' | 'evening',
        activity,
        status as 'not_required' | 'pending' | 'booked' | 'waitlist',
        refOpt,
        validatedBookBy
      );
      await sm.saveWithTracking('set-activity-booking', `D${dayNumber} ${session} ${activity}`);
      console.log('✅ Activity booking status updated');
    } else {
      console.log('🔸 DRY RUN - no changes saved');
    }
  },
};

registerCommand(setActivityBookingCommand);

// ── set-activity-title ─────────────────────────────────────────────

const setActivityTitleCommand: CommandHandler = {
  names: ['set-activity-title'],
  description: 'Rename an activity within a day/session.',
  usage: 'set-activity-title <day> <session> <activity> <new_title> [--dest <slug>]',
  async execute(ctx: CliContext): Promise<void> {
    const { sm, args, dryRun } = ctx;
    const [, dayStr, session, activity, newTitle] = args.cleanArgs;
    const destOpt = args.optionValue('--dest');

    if (!dayStr || !session || !activity || !newTitle) {
      console.error('Error: set-activity-title requires <day> <session> <activity> <new_title>');
      console.error('Example: set-activity-title 2 morning "Fushimi Inari" "金閣寺 (Kinkaku-ji)"');
      process.exit(1);
    }

    const dayNumber = requireDayArg(dayStr);
    requireSessionArg(session);

    if (!newTitle.trim()) {
      console.error('Error: <new_title> cannot be empty');
      process.exit(1);
    }

    const destination = sm.resolveDestination(destOpt);

    console.log(`\n✏️  Renaming activity:`);
    console.log(`   Destination: ${destination}`);
    console.log(`   Day ${dayNumber} ${session}: "${activity}"`);
    console.log(`   New title: "${newTitle}"`);

    if (!dryRun) {
      sm.setActivityTitle(
        destination,
        dayNumber,
        session as 'morning' | 'noon' | 'afternoon' | 'evening',
        activity,
        newTitle
      );
      await sm.saveWithTracking('set-activity-title', `D${dayNumber} ${session} ${activity}`);
      console.log('✅ Activity title updated');
    } else {
      console.log('🔸 DRY RUN - no changes saved');
    }
  },
};

registerCommand(setActivityTitleCommand);
