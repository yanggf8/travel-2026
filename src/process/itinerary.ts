/**
 * Daily Itinerary Process Tool
 * Commands: status, add, remove, update, set-transit
 */

import {
  ProcessContext,
  ProcessResult,
  Activity,
  DayPlan,
} from './types';
import {
  createContext,
  commitChanges,
  generateId,
  formatResult,
  syncItineraryStatus,
  runCli,
} from './plan-updater';

type SessionType = 'morning' | 'afternoon' | 'evening';

interface AddActivityInput {
  day_number: number;
  session: SessionType;
  name: string;
  type?: string;
  location?: {
    name?: string;
    address?: string;
    coordinates?: { lat: number; lng: number };
    nearest_station?: string;
    walking_minutes_from_station?: number;
  };
  time?: {
    start?: string;
    end?: string;
    duration_minutes?: number;
    flexible?: boolean;
  };
  cost?: {
    admission?: number;
    estimated_spending?: number;
  };
  booking?: {
    required?: boolean;
    booking_url?: string;
  };
  notes?: string;
  priority?: string;
  weather_dependent?: boolean;
}

interface RemoveActivityInput {
  day_number: number;
  session: SessionType;
  activity_id: string;
}

interface UpdateActivityInput {
  day_number: number;
  session: SessionType;
  activity_id: string;
  updates: Partial<AddActivityInput>;
}

interface SetTransitInput {
  day_number: number;
  session: SessionType;
  activity_id: string;
  transit: {
    method: string;
    duration_minutes: number;
    cost?: number;
    route?: string;
  };
}

function findDay(ctx: ProcessContext, dayNumber: number): DayPlan | null {
  return ctx.plan.process_5_daily_itinerary.days.find(
    (d) => d.day_number === dayNumber
  ) ?? null;
}

function syncDayStatus(day: DayPlan): void {
  const hasMorning = day.morning.activities.length >= 1;
  const hasAfternoon = day.afternoon.activities.length >= 1;

  if (!hasMorning || !hasAfternoon) {
    day.status = 'pending';
    return;
  }

  // Check if all activities are confirmed (have name + location + time)
  const allConfirmed = [...day.morning.activities, ...day.afternoon.activities].every(
    (a) => a.name && a.location?.name && a.time?.start
  );

  if (allConfirmed) {
    day.status = 'confirmed';
    return;
  }

  // Check if all activities have names
  const allNamed = [...day.morning.activities, ...day.afternoon.activities].every(
    (a) => a.name
  );

  if (allNamed) {
    day.status = 'selected';
    return;
  }

  day.status = 'researched';
}

export function addActivity(
  ctx: ProcessContext,
  input: AddActivityInput
): ProcessResult {
  const result: ProcessResult = {
    success: false,
    processName: 'Daily Itinerary',
    action: `Add activity to Day ${input.day_number} ${input.session}`,
    changes: [],
    errors: [],
  };

  try {
    const day = findDay(ctx, input.day_number);
    if (!day) {
      result.errors.push(`Day ${input.day_number} not found`);
      return result;
    }

    const session = day[input.session];
    if (!session) {
      result.errors.push(`Session ${input.session} not found`);
      return result;
    }

    const activity: Activity = {
      id: generateId('activity'),
      name: input.name,
      type: input.type ?? null,
      location: {
        name: input.location?.name ?? null,
        address: input.location?.address ?? null,
        coordinates: input.location?.coordinates ?? null,
        nearest_station: input.location?.nearest_station ?? null,
        walking_minutes_from_station: input.location?.walking_minutes_from_station ?? null,
      },
      time: {
        start: input.time?.start ?? null,
        end: input.time?.end ?? null,
        duration_minutes: input.time?.duration_minutes ?? null,
        flexible: input.time?.flexible ?? false,
      },
      cost: {
        admission: input.cost?.admission ?? null,
        estimated_spending: input.cost?.estimated_spending ?? null,
        currency: 'JPY',
      },
      booking: {
        required: input.booking?.required ?? false,
        booking_ref: null,
        booking_url: input.booking?.booking_url ?? null,
        booked: false,
      },
      transit_to_next: {
        method: null,
        duration_minutes: null,
        cost: null,
        route: null,
      },
      notes: input.notes ?? null,
      priority: input.priority ?? null,
      weather_dependent: input.weather_dependent ?? false,
    };

    session.activities.push(activity);

    // Sync day status
    syncDayStatus(day);
    result.changes.push(`Day ${input.day_number} status: ${day.status}`);

    // Sync overall itinerary status
    const newStatus = syncItineraryStatus(ctx);
    result.changes.push(`Process status: ${newStatus}`);

    result.changes.push(
      `Added activity: ${input.name} to Day ${input.day_number} ${input.session} (${activity.id})`
    );
    result.success = true;
  } catch (err) {
    result.errors.push(`Failed to add activity: ${err}`);
  }

  return result;
}

export function removeActivity(
  ctx: ProcessContext,
  input: RemoveActivityInput
): ProcessResult {
  const result: ProcessResult = {
    success: false,
    processName: 'Daily Itinerary',
    action: `Remove activity from Day ${input.day_number} ${input.session}`,
    changes: [],
    errors: [],
  };

  try {
    const day = findDay(ctx, input.day_number);
    if (!day) {
      result.errors.push(`Day ${input.day_number} not found`);
      return result;
    }

    const session = day[input.session];
    const idx = session.activities.findIndex((a) => a.id === input.activity_id);

    if (idx === -1) {
      result.errors.push(`Activity not found: ${input.activity_id}`);
      return result;
    }

    const removed = session.activities.splice(idx, 1)[0];

    // Sync day status
    syncDayStatus(day);
    result.changes.push(`Day ${input.day_number} status: ${day.status}`);

    // Sync overall itinerary status
    const newStatus = syncItineraryStatus(ctx);
    result.changes.push(`Process status: ${newStatus}`);

    result.changes.push(`Removed activity: ${removed.name} (${removed.id})`);
    result.success = true;
  } catch (err) {
    result.errors.push(`Failed to remove activity: ${err}`);
  }

  return result;
}

export function updateActivity(
  ctx: ProcessContext,
  input: UpdateActivityInput
): ProcessResult {
  const result: ProcessResult = {
    success: false,
    processName: 'Daily Itinerary',
    action: `Update activity in Day ${input.day_number} ${input.session}`,
    changes: [],
    errors: [],
  };

  try {
    const day = findDay(ctx, input.day_number);
    if (!day) {
      result.errors.push(`Day ${input.day_number} not found`);
      return result;
    }

    const session = day[input.session];
    const activity = session.activities.find((a) => a.id === input.activity_id);

    if (!activity) {
      result.errors.push(`Activity not found: ${input.activity_id}`);
      return result;
    }

    const updates = input.updates;

    if (updates.name !== undefined) {
      activity.name = updates.name;
      result.changes.push(`Updated name: ${updates.name}`);
    }
    if (updates.type !== undefined) {
      activity.type = updates.type;
      result.changes.push(`Updated type: ${updates.type}`);
    }
    if (updates.location) {
      Object.assign(activity.location, updates.location);
      result.changes.push(`Updated location`);
    }
    if (updates.time) {
      Object.assign(activity.time, updates.time);
      result.changes.push(`Updated time`);
    }
    if (updates.cost) {
      Object.assign(activity.cost, updates.cost);
      result.changes.push(`Updated cost`);
    }
    if (updates.booking) {
      Object.assign(activity.booking, updates.booking);
      result.changes.push(`Updated booking`);
    }
    if (updates.notes !== undefined) {
      activity.notes = updates.notes;
      result.changes.push(`Updated notes`);
    }
    if (updates.priority !== undefined) {
      activity.priority = updates.priority;
      result.changes.push(`Updated priority: ${updates.priority}`);
    }
    if (updates.weather_dependent !== undefined) {
      activity.weather_dependent = updates.weather_dependent;
      result.changes.push(`Updated weather_dependent: ${updates.weather_dependent}`);
    }

    // Sync day status
    syncDayStatus(day);
    result.changes.push(`Day ${input.day_number} status: ${day.status}`);

    // Sync overall itinerary status
    const newStatus = syncItineraryStatus(ctx);
    result.changes.push(`Process status: ${newStatus}`);

    result.success = true;
  } catch (err) {
    result.errors.push(`Failed to update activity: ${err}`);
  }

  return result;
}

export function setTransit(
  ctx: ProcessContext,
  input: SetTransitInput
): ProcessResult {
  const result: ProcessResult = {
    success: false,
    processName: 'Daily Itinerary',
    action: `Set transit for activity in Day ${input.day_number}`,
    changes: [],
    errors: [],
  };

  try {
    const day = findDay(ctx, input.day_number);
    if (!day) {
      result.errors.push(`Day ${input.day_number} not found`);
      return result;
    }

    const session = day[input.session];
    const activity = session.activities.find((a) => a.id === input.activity_id);

    if (!activity) {
      result.errors.push(`Activity not found: ${input.activity_id}`);
      return result;
    }

    activity.transit_to_next = {
      method: input.transit.method,
      duration_minutes: input.transit.duration_minutes,
      cost: input.transit.cost ?? null,
      route: input.transit.route ?? null,
    };

    result.changes.push(
      `Set transit: ${input.transit.method} (${input.transit.duration_minutes} min)`
    );
    result.success = true;
  } catch (err) {
    result.errors.push(`Failed to set transit: ${err}`);
  }

  return result;
}

export function getStatus(ctx: ProcessContext): string {
  const itinerary = ctx.plan.process_5_daily_itinerary;
  const lines: string[] = [];

  lines.push('=== Daily Itinerary Status ===');
  lines.push(`Overall: ${itinerary.status}`);
  lines.push('');

  for (const day of itinerary.days) {
    const morningCount = day.morning.activities.length;
    const afternoonCount = day.afternoon.activities.length;
    const eveningCount = day.evening.activities.length;
    const total = morningCount + afternoonCount + eveningCount;

    lines.push(`Day ${day.day_number} (${day.date}) - ${day.day_type}`);
    lines.push(`  Status: ${day.status}`);
    lines.push(`  Activities: ${total} (M:${morningCount} A:${afternoonCount} E:${eveningCount})`);

    if (morningCount > 0) {
      lines.push(`  Morning:`);
      for (const act of day.morning.activities) {
        lines.push(`    - ${act.name} ${act.time?.start ? `@ ${act.time.start}` : ''}`);
      }
    }

    if (afternoonCount > 0) {
      lines.push(`  Afternoon:`);
      for (const act of day.afternoon.activities) {
        lines.push(`    - ${act.name} ${act.time?.start ? `@ ${act.time.start}` : ''}`);
      }
    }

    if (eveningCount > 0) {
      lines.push(`  Evening:`);
      for (const act of day.evening.activities) {
        lines.push(`    - ${act.name} ${act.time?.start ? `@ ${act.time.start}` : ''}`);
      }
    }

    lines.push('');
  }

  return lines.join('\n');
}

// CLI interface
function printUsage(): void {
  console.log('Daily Itinerary Process Tool');
  console.log('');
  console.log('Usage: ts-node src/process/itinerary.ts <command> [options]');
  console.log('');
  console.log('Commands:');
  console.log('  status                    Show itinerary status');
  console.log('  add <json>                Add activity (JSON input)');
  console.log('  remove <json>             Remove activity (JSON: {day_number, session, activity_id})');
  console.log('  update <json>             Update activity (JSON: {day_number, session, activity_id, updates})');
  console.log('  set-transit <json>        Set transit to next activity');
  console.log('');
  console.log('Options:');
  console.log('  --file <path>             Path to travel-plan.json');
  console.log('  --dry-run                 Show changes without saving');
}

function cliMain(): void {
  const args = process.argv.slice(2);

  if (args.length === 0 || args[0] === '--help' || args[0] === '-h') {
    printUsage();
    process.exit(0);
  }

  // Parse options
  let planPath = 'data/travel-plan.json';
  let dryRun = false;
  const fileIdx = args.indexOf('--file');
  if (fileIdx !== -1) {
    if (!args[fileIdx + 1] || args[fileIdx + 1].startsWith('--')) {
      console.error('Error: --file requires a path argument');
      printUsage();
      process.exit(1);
    }
    planPath = args[fileIdx + 1];
    args.splice(fileIdx, 2);
  }
  if (args.includes('--dry-run')) {
    dryRun = true;
    args.splice(args.indexOf('--dry-run'), 1);
  }

  const command = args[0];
  const ctx = createContext(planPath);

  switch (command) {
    case 'status': {
      console.log(getStatus(ctx));
      break;
    }

    case 'add': {
      if (!args[1]) {
        console.error('Error: add requires JSON input');
        printUsage();
        process.exit(1);
      }
      const input = JSON.parse(args[1]) as AddActivityInput;
      const result = addActivity(ctx, input);
      console.log(formatResult(result));
      if (result.success && !dryRun) {
        commitChanges(ctx);
        console.log('\nChanges saved.');
      }
      break;
    }

    case 'remove': {
      if (!args[1]) {
        console.error('Error: remove requires JSON input');
        printUsage();
        process.exit(1);
      }
      const input = JSON.parse(args[1]) as RemoveActivityInput;
      const result = removeActivity(ctx, input);
      console.log(formatResult(result));
      if (result.success && !dryRun) {
        commitChanges(ctx);
        console.log('\nChanges saved.');
      }
      break;
    }

    case 'update': {
      if (!args[1]) {
        console.error('Error: update requires JSON input');
        printUsage();
        process.exit(1);
      }
      const input = JSON.parse(args[1]) as UpdateActivityInput;
      const result = updateActivity(ctx, input);
      console.log(formatResult(result));
      if (result.success && !dryRun) {
        commitChanges(ctx);
        console.log('\nChanges saved.');
      }
      break;
    }

    case 'set-transit': {
      if (!args[1]) {
        console.error('Error: set-transit requires JSON input');
        printUsage();
        process.exit(1);
      }
      const input = JSON.parse(args[1]) as SetTransitInput;
      const result = setTransit(ctx, input);
      console.log(formatResult(result));
      if (result.success && !dryRun) {
        commitChanges(ctx);
        console.log('\nChanges saved.');
      }
      break;
    }

    default:
      console.error(`Unknown command: ${command}`);
      printUsage();
      process.exit(1);
  }
}

runCli(cliMain);
