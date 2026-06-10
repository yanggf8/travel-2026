import type { CommandHandler, CliContext } from '../shared/types';
import { registerCommand } from './registry';
import { requireDayArg } from '../shared/validation';

// ── set-route-segment ──────────────────────────────────────────────

const setRouteSegmentCommand: CommandHandler = {
  names: ['set-route-segment'],
  description: 'Set a single route segment for a day.',
  usage: 'set-route-segment <day> <sort_order> <from> <to> <mode> [--duration <min>] [--notes "..."] [--start-time HH:MM] [--dest <slug>]',
  async execute(ctx: CliContext): Promise<void> {
    const { sm, args, dryRun } = ctx;
    const [, dayStr, sortOrderStr, fromPlace, toPlace, mode] = args.cleanArgs;
    const destOpt = args.optionValue('--dest');
    const durationOpt = args.optionValue('--duration');
    const notesOpt = args.optionValue('--notes');
    const startTimeOpt = args.optionValue('--start-time');

    if (!dayStr || !sortOrderStr || !fromPlace || !toPlace || !mode) {
      console.error('Error: set-route-segment requires <day> <sort_order> <from> <to> <mode>');
      console.error('Example: set-route-segment 1 0 "關渡" "嘟嘟房桃園機場貨運1站" driving --duration 45');
      process.exit(1);
    }

    const dayNumber = requireDayArg(dayStr);

    const sortOrder = parseInt(sortOrderStr, 10);
    if (isNaN(sortOrder) || sortOrder < 0) {
      console.error('Error: <sort_order> must be a non-negative integer (0-based)');
      process.exit(1);
    }

    const validModes = ['transit', 'walking', 'driving'];
    if (!validModes.includes(mode)) {
      console.error('Error: <mode> must be one of: transit | walking | driving');
      process.exit(1);
    }

    const durationMin = durationOpt ? parseInt(durationOpt, 10) : undefined;
    if (durationOpt && isNaN(durationMin!)) {
      console.error('Error: --duration must be a number (minutes)');
      process.exit(1);
    }

    const destination = sm.resolveDestination(destOpt);

    console.log(`\n🗺️  Setting route segment:`);
    console.log(`   Destination: ${destination}`);
    console.log(`   Day ${dayNumber} slot ${sortOrder}: ${fromPlace} → ${toPlace} (${mode}${durationMin != null ? `, ${durationMin} min` : ''}${startTimeOpt ? `, start ${startTimeOpt}` : ''})`);
    if (notesOpt) console.log(`   Notes: ${notesOpt}`);

    if (!dryRun) {
      sm.setRouteSegment(destination, dayNumber, sortOrder, fromPlace, toPlace, mode as 'transit' | 'walking' | 'driving', durationMin, notesOpt ?? undefined, startTimeOpt ?? undefined);
      await sm.saveWithTracking('set-route-segment', `D${dayNumber} slot${sortOrder} ${fromPlace}→${toPlace}`);
      console.log('✅ Route segment updated');
    } else {
      console.log('🔸 DRY RUN - no changes saved');
    }
  },
};

registerCommand(setRouteSegmentCommand);

// ── set-route-segments-bulk ────────────────────────────────────────

const setRouteSegmentsBulkCommand: CommandHandler = {
  names: ['set-route-segments-bulk'],
  description: 'Replace all route segments for a day from a JSON array.',
  usage: 'set-route-segments-bulk <day> --json \'[{"from":"A","to":"B","mode":"walking","duration":5},...]\' [--dest <slug>]',
  async execute(ctx: CliContext): Promise<void> {
    const { sm, args, dryRun } = ctx;
    const [, dayStr] = args.cleanArgs;
    const destOpt = args.optionValue('--dest');

    if (!dayStr) {
      console.error('Error: set-route-segments-bulk requires <day> --json \'[...]\'');
      console.error('  Each segment: {"from":"place","to":"place","mode":"transit|walking|driving","duration":N,"notes":"...","start_time":"HH:MM"}');
      console.error('Example: set-route-segments-bulk 4 --json \'[{"from":"hotel","to":"京都駅","mode":"walking","duration":3}]\'');
      process.exit(1);
    }

    const dayNumber = requireDayArg(dayStr);

    const jsonOpt = args.optionValue('--json');
    if (!jsonOpt) {
      console.error('Error: --json is required with a JSON array of route segments');
      process.exit(1);
    }

    let segments: Array<{ from: string; to: string; mode: 'transit' | 'walking' | 'driving'; duration?: number; notes?: string; start_time?: string }>;
    try {
      segments = JSON.parse(jsonOpt);
      if (!Array.isArray(segments)) throw new Error('Not an array');
      for (const s of segments) {
        if (!s.from || !s.to || !s.mode) throw new Error('Each segment needs from, to, mode');
        if (!['transit', 'walking', 'driving'].includes(s.mode)) throw new Error(`Invalid mode: ${s.mode}`);
      }
    } catch (e) {
      console.error(`Error: Invalid --json: ${(e as Error).message}`);
      process.exit(1);
    }

    const destination = sm.resolveDestination(destOpt);
    console.log(`\n🛤️  Replacing all route segments for Day ${dayNumber}:`);
    console.log(`   Destination: ${destination}`);
    console.log(`   ${segments.length} segments`);
    for (const s of segments) {
      console.log(`   ${s.from} → ${s.to} (${s.mode}${s.duration ? `, ${s.duration}min` : ''})`);
    }

    if (!dryRun) {
      sm.setRouteSegmentsBulk(destination, dayNumber, segments);
      await sm.saveWithTracking('set-route-segments-bulk', `D${dayNumber} ${segments.length} segments`);
      console.log('✅ Route segments replaced');
    } else {
      console.log('🔸 DRY RUN - no changes saved');
    }
  },
};

registerCommand(setRouteSegmentsBulkCommand);
