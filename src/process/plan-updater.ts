/**
 * Plan Updater - Safe read/write operations for travel-plan.json
 */

import * as fs from 'fs';
import * as path from 'path';
import * as os from 'os';
import { TravelPlan, ProcessContext, ProcessResult } from './types';

export function loadPlan(planPath: string): TravelPlan {
  const resolvedPath = path.isAbsolute(planPath)
    ? planPath
    : path.resolve(process.cwd(), planPath);

  if (!fs.existsSync(resolvedPath)) {
    throw new Error(`Plan file not found: ${resolvedPath}`);
  }

  const content = fs.readFileSync(resolvedPath, 'utf-8');
  return JSON.parse(content) as TravelPlan;
}

/**
 * Atomic save - writes to temp file then renames
 */
export function savePlan(planPath: string, plan: TravelPlan): void {
  const resolvedPath = path.isAbsolute(planPath)
    ? planPath
    : path.resolve(process.cwd(), planPath);

  const content = JSON.stringify(plan, null, 2);
  const dir = path.dirname(resolvedPath);
  const tempPath = path.join(dir, `.travel-plan.tmp.${process.pid}`);

  try {
    fs.writeFileSync(tempPath, content, 'utf-8');
    fs.renameSync(tempPath, resolvedPath);
  } catch (err) {
    // Clean up temp file if rename failed
    try {
      if (fs.existsSync(tempPath)) {
        fs.unlinkSync(tempPath);
      }
    } catch {
      // Ignore cleanup errors
    }
    throw err;
  }
}

export function createContext(planPath: string = 'data/travel-plan.json'): ProcessContext {
  const plan = loadPlan(planPath);
  return { planPath, plan };
}

export function commitChanges(ctx: ProcessContext): void {
  savePlan(ctx.planPath, ctx.plan);
}

/**
 * Generate a unique ID for candidates
 */
export function generateId(prefix: string): string {
  const timestamp = Date.now().toString(36);
  const random = Math.random().toString(36).substring(2, 6);
  return `${prefix}_${timestamp}_${random}`;
}

/**
 * Sync process_3_transportation.status based on sub-statuses
 * Bubble-up: highest achieved status across all sub-components
 */
export function syncTransportationStatus(ctx: ProcessContext): string {
  const t = ctx.plan.process_3_transportation;

  // Check if booked
  const flightBooked = t.flight.outbound.booking_ref && t.flight.return.booking_ref;
  if (flightBooked) {
    t.status = 'booked';
    return 'booked';
  }

  // Check if selected (outbound flight selected is minimum)
  const flightSelected = t.flight.outbound.airline !== null;
  if (flightSelected) {
    t.status = 'selected';
    return 'selected';
  }

  // Check if researched (any candidates exist)
  const hasAnyCandidates =
    t.flight.candidates.length > 0 ||
    t.home_to_airport.candidates.length > 0 ||
    t.airport_to_hotel.candidates.length > 0;
  if (hasAnyCandidates) {
    t.status = 'researched';
    return 'researched';
  }

  t.status = 'pending';
  return 'pending';
}

/**
 * Sync process_4_accommodation.status based on sub-statuses
 */
export function syncAccommodationStatus(ctx: ProcessContext): string {
  const a = ctx.plan.process_4_accommodation;

  // Check if booked
  if (a.hotel.booking.confirmation_number) {
    a.status = 'booked';
    return 'booked';
  }

  // Check if hotel selected
  if (a.hotel.selected_hotel) {
    a.status = 'selected';
    return 'selected';
  }

  // Check if researched (zone or hotel candidates exist)
  const hasAnyCandidates =
    a.location_zone.candidates.length > 0 ||
    a.hotel.candidates.length > 0;
  if (hasAnyCandidates) {
    a.status = 'researched';
    return 'researched';
  }

  a.status = 'pending';
  return 'pending';
}

/**
 * Sync process_5_daily_itinerary.status based on day statuses
 */
export function syncItineraryStatus(ctx: ProcessContext): string {
  const it = ctx.plan.process_5_daily_itinerary;

  // Check if all days have activities
  const allDaysHaveActivities = it.days.every(
    (d) => d.morning.activities.length >= 1 && d.afternoon.activities.length >= 1
  );

  // Check if all activities are confirmed (have name + location + time)
  const allActivitiesConfirmed = it.days.every((d) => {
    const morningOk = d.morning.activities.every(
      (a) => a.name && a.location?.name && a.time?.start
    );
    const afternoonOk = d.afternoon.activities.every(
      (a) => a.name && a.location?.name && a.time?.start
    );
    return morningOk && afternoonOk;
  });

  if (allDaysHaveActivities && allActivitiesConfirmed) {
    it.status = 'confirmed';
    return 'confirmed';
  }

  // Check if all activities have names (selected)
  const allActivitiesNamed = it.days.every((d) => {
    const morningOk = d.morning.activities.every((a) => a.name);
    const afternoonOk = d.afternoon.activities.every((a) => a.name);
    return morningOk && afternoonOk;
  });

  if (allDaysHaveActivities && allActivitiesNamed) {
    it.status = 'selected';
    return 'selected';
  }

  if (allDaysHaveActivities) {
    it.status = 'researched';
    return 'researched';
  }

  it.status = 'pending';
  return 'pending';
}

/**
 * Format result for agent output
 */
export function formatResult(result: ProcessResult): string {
  const lines: string[] = [];

  lines.push(`=== Process: ${result.processName} ===`);
  lines.push(`Action: ${result.action}`);
  lines.push(`Success: ${result.success}`);

  if (result.changes.length > 0) {
    lines.push('');
    lines.push('Changes:');
    result.changes.forEach(c => lines.push(`  + ${c}`));
  }

  if (result.errors.length > 0) {
    lines.push('');
    lines.push('Errors:');
    result.errors.forEach(e => lines.push(`  ! ${e}`));
  }

  return lines.join('\n');
}

/**
 * Safe CLI wrapper - catches errors and prints friendly messages
 */
export function runCli(fn: () => void): void {
  try {
    fn();
  } catch (err) {
    if (err instanceof SyntaxError) {
      console.error('Error: Invalid JSON input');
      console.error(`  ${err.message}`);
    } else if (err instanceof Error) {
      console.error(`Error: ${err.message}`);
    } else {
      console.error('Error: Unknown error occurred');
    }
    process.exit(1);
  }
}
