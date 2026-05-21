import type { CommandHandler, CliContext } from '../shared/types';
import { registerCommand } from './registry';
import { defaultValidator } from '../../validation/itinerary-validator';
import type { DaySummary, IssueSeverity, ResolvedActivity } from '../../validation/types';

// ── helpers (extracted from travel-update.ts) ────────────────────────

function severityRank(sev: IssueSeverity): number {
  if (sev === 'error') return 3;
  if (sev === 'warning') return 2;
  return 1;
}

function parseSeverity(value: string | undefined): IssueSeverity {
  if (!value) return 'info';
  const v = value.toLowerCase();
  if (v === 'error' || v === 'warning' || v === 'info') return v;
  console.error('Error: --severity must be one of: error | warning | info');
  process.exit(1);
}

function parseOperatingHoursFromNotes(notes: string | null | undefined): string | undefined {
  if (!notes) return undefined;
  const match = notes.match(/(?:^|\s)Hours:\s*([^|\n]+)/i);
  if (!match) return undefined;
  const v = match[1].trim();
  return v || undefined;
}

function inferDurationMin(activity: Record<string, unknown>): number {
  const duration = activity.duration_min as number | null | undefined;
  if (typeof duration === 'number' && Number.isFinite(duration) && duration > 0) return duration;

  const start = activity.start_time as string | undefined;
  const end = activity.end_time as string | undefined;
  if (start && end) {
    const parse = (t: string): number | null => {
      const m = t.match(/^(\d{1,2}):(\d{2})$/);
      if (!m) return null;
      const hh = parseInt(m[1], 10);
      const mm = parseInt(m[2], 10);
      if (!Number.isFinite(hh) || !Number.isFinite(mm)) return null;
      return hh * 60 + mm;
    };
    const s = parse(start);
    const e = parse(end);
    if (s !== null && e !== null && e > s) return e - s;
  }

  return 60;
}

function buildDaySummaries(days: Array<Record<string, unknown>>): DaySummary[] {
  const summaries: DaySummary[] = [];
  for (const day of days) {
    const dayNumber = day.day_number as number;
    const date = (day.date as string) || '';
    const theme = (day.theme as string | null) || (day.day_type as string) || '';

    const activities: ResolvedActivity[] = [];
    const areas = new Set<string>();
    let totalDurationMin = 0;
    let fixedTimeCount = 0;
    let pendingBookings = 0;

    for (const sessionName of ['morning', 'noon', 'afternoon', 'evening'] as const) {
      const session = day[sessionName] as Record<string, unknown> | undefined;
      const acts = (session?.activities as Array<unknown> | undefined) ?? [];
      for (const act of acts) {
        if (typeof act === 'string') {
          const durationMin = 60;
          activities.push({
            id: `legacy_${dayNumber}_${sessionName}_${activities.length}`,
            title: act,
            day: dayNumber,
            session: sessionName,
            durationMin,
            isFixedTime: false,
            bookingRequired: false,
          });
          totalDurationMin += durationMin;
          continue;
        }

        const a = act as Record<string, unknown>;
        const id = (a.id as string) || `activity_${dayNumber}_${sessionName}_${activities.length}`;
        const title = (a.title as string) || '';
        const area = (a.area as string) || undefined;
        if (area) areas.add(area);

        const durationMin = inferDurationMin(a);
        totalDurationMin += durationMin;

        const isFixedTime = Boolean(a.is_fixed_time);
        if (isFixedTime) fixedTimeCount++;

        const bookingRequired = Boolean(a.booking_required);
        const bookingStatus = a.booking_status as string | undefined;
        const bookByDate = a.book_by as string | undefined;
        if (bookingRequired && bookingStatus !== 'booked') pendingBookings++;

        activities.push({
          id,
          title,
          day: dayNumber,
          session: sessionName,
          startTime: a.start_time as string | undefined,
          endTime: a.end_time as string | undefined,
          durationMin,
          isFixedTime,
          area,
          bookingRequired,
          bookingStatus,
          bookByDate,
          operatingHours: parseOperatingHoursFromNotes(a.notes as string | null | undefined),
        });
      }
    }

    summaries.push({
      dayNumber,
      date,
      theme,
      activities,
      areas: Array.from(areas),
      totalDurationMin,
      fixedTimeCount,
      pendingBookings,
    });
  }
  return summaries;
}

function printValidationResult(destination: string, result: { valid: boolean; summary: any; issues: any[] }, threshold: IssueSeverity): void {
  const status = result.valid ? 'VALID' : 'ISSUES FOUND';
  console.log(`\nvalidate-itinerary (${destination})`);
  console.log(`   Result: ${status}`);
  console.log(`   Showing: ${threshold}+`);
  console.log(`   Summary: ${result.summary.errors} error(s), ${result.summary.warnings} warning(s), ${result.summary.info} info`);

  if (result.issues.length === 0) {
    console.log('\n(no issues to show)\n');
    return;
  }

  console.log('\nIssues:');
  for (const i of result.issues) {
    const where = [
      typeof i.day === 'number' ? `Day ${i.day}` : null,
      i.session ? `${i.session}` : null,
    ].filter(Boolean).join(' ');
    const prefix = i.severity === 'error' ? 'X' : i.severity === 'warning' ? '!' : 'i';
    console.log(`  ${prefix} ${where ? `${where}: ` : ''}${i.message}`);
    if (i.suggestion) console.log(`     -> ${i.suggestion}`);
  }
  console.log('');
}

// ── validate-itinerary ───────────────────────────────────────────────

const validateItineraryCommand: CommandHandler = {
  names: ['validate-itinerary'],
  description: 'Validate itinerary for issues (overlaps, missing bookings, etc.).',
  usage: 'validate-itinerary [--dest <slug>] [--severity error|warning|info] [--json]',
  async execute(ctx: CliContext): Promise<void> {
    const { sm, args } = ctx;
    const destOpt = args.optionValue('--dest');
    const severityOpt = args.optionValue('--severity');
    const jsonOpt = args.hasFlag('--json');

    const destination = sm.resolveDestination(destOpt);
    const plan = sm.getPlan();
    const destObj = plan.destinations[destination] as Record<string, unknown> | undefined;
    if (!destObj) {
      console.error(`Error: Destination not found: ${destination}`);
      process.exit(1);
    }

    const p5 = destObj.process_5_daily_itinerary as Record<string, unknown> | undefined;
    const days = (p5?.days as Array<Record<string, unknown>> | undefined) ?? [];
    if (days.length === 0) {
      console.error('Error: No itinerary days found. Run scaffold-itinerary first.');
      process.exit(1);
    }

    const daySummaries = buildDaySummaries(days);
    const result = defaultValidator.validate(daySummaries, new Date());

    const threshold: IssueSeverity = parseSeverity(severityOpt);
    const filtered = {
      ...result,
      issues: result.issues.filter((i) => severityRank(i.severity) >= severityRank(threshold)),
    };

    if (jsonOpt) {
      console.log(JSON.stringify(filtered, null, 2));
    } else {
      printValidationResult(destination, filtered, threshold);
    }

    process.exitCode = filtered.valid ? 0 : 1;
  },
};

registerCommand(validateItineraryCommand);
