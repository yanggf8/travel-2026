#!/usr/bin/env ts-node
/**
 * Project Initializer
 *
 * Scaffolds a new travel plan with proper structure.
 * Can be used to start a fresh plan or clone from template.
 *
 * Usage:
 *   npx ts-node src/templates/project-init.ts --dest tokyo_2026 --dates 2026-02-13 2026-02-17
 *   npx ts-node src/templates/project-init.ts --from-template japan_template --dest osaka_2026
 */

import * as fs from 'fs';
import * as path from 'path';
import { getDestinationConfig, getAvailableDestinations } from '../config/loader';
import { DEFAULTS } from '../config/constants';

export interface ProjectInitOptions {
  destination: string;
  startDate: string;
  endDate: string;
  pax?: number;
  outputDir?: string;
  templateId?: string;
}

export interface TravelPlanTemplate {
  schema_version: string;
  active_destination: string;
  process_1_date_anchor: {
    status: string;
    start_date: string;
    end_date: string;
    duration_days: number;
    flexible: boolean;
    updated_at: string;
  };
  destinations: Record<string, DestinationPlanTemplate>;
  cascade_state: {
    destinations: Record<string, Record<string, { dirty: boolean; reason: string | null }>>;
  };
}

export interface DestinationPlanTemplate {
  process_2_destination: {
    status: string;
    slug: string;
    display_name: string;
    updated_at: string;
  };
  process_3_4_packages: {
    status: string;
    pax: number;
    results: { offers: unknown[]; provenance: unknown[] };
    updated_at: string;
  };
  process_3_transportation: {
    status: string;
    flight: null;
    airport_transfers: { arrival: null; departure: null };
    updated_at: string;
  };
  process_4_accommodation: {
    status: string;
    hotel: null;
    updated_at: string;
  };
  process_5_daily_itinerary: {
    status: string;
    pace: string;
    days: DayTemplate[];
    updated_at: string;
  };
}

export interface DayTemplate {
  day_number: number;
  date: string;
  theme: string;
  morning: { time_range: null; activities: unknown[] };
  afternoon: { time_range: null; activities: unknown[] };
  evening: { time_range: null; activities: unknown[] };
}

function calculateDurationDays(start: string, end: string): number {
  const startDate = new Date(start);
  const endDate = new Date(end);
  return Math.ceil((endDate.getTime() - startDate.getTime()) / (1000 * 60 * 60 * 24)) + 1;
}

function generateDays(startDate: string, endDate: string): DayTemplate[] {
  const days: DayTemplate[] = [];
  const start = new Date(startDate);
  const duration = calculateDurationDays(startDate, endDate);

  for (let i = 0; i < duration; i++) {
    const date = new Date(start);
    date.setDate(start.getDate() + i);
    const dateStr = date.toISOString().split('T')[0];

    days.push({
      day_number: i + 1,
      date: dateStr,
      theme: i === 0 ? 'Arrival' : i === duration - 1 ? 'Departure' : 'TBD',
      morning: { time_range: null, activities: [] },
      afternoon: { time_range: null, activities: [] },
      evening: { time_range: null, activities: [] },
    });
  }

  return days;
}

export function createTravelPlan(options: ProjectInitOptions): TravelPlanTemplate {
  const now = new Date().toISOString();
  const destConfig = getDestinationConfig(options.destination);

  if (!destConfig) {
    const available = getAvailableDestinations();
    throw new Error(
      `Unknown destination: ${options.destination}. Available: ${available.join(', ')}`
    );
  }

  const duration = calculateDurationDays(options.startDate, options.endDate);
  const pax = options.pax || DEFAULTS.pax;

  const destPlan: DestinationPlanTemplate = {
    process_2_destination: {
      status: 'confirmed',
      slug: options.destination,
      display_name: destConfig.display_name,
      updated_at: now,
    },
    process_3_4_packages: {
      status: 'pending',
      pax,
      results: { offers: [], provenance: [] },
      updated_at: now,
    },
    process_3_transportation: {
      status: 'pending',
      flight: null,
      airport_transfers: { arrival: null, departure: null },
      updated_at: now,
    },
    process_4_accommodation: {
      status: 'pending',
      hotel: null,
      updated_at: now,
    },
    process_5_daily_itinerary: {
      status: 'pending',
      pace: DEFAULTS.pace,
      days: generateDays(options.startDate, options.endDate),
      updated_at: now,
    },
  };

  return {
    schema_version: '4.2.0',
    active_destination: options.destination,
    process_1_date_anchor: {
      status: 'confirmed',
      start_date: options.startDate,
      end_date: options.endDate,
      duration_days: duration,
      flexible: false,
      updated_at: now,
    },
    destinations: {
      [options.destination]: destPlan,
    },
    cascade_state: {
      destinations: {
        [options.destination]: {
          process_3_4_packages: { dirty: false, reason: null },
          process_3_transportation: { dirty: false, reason: null },
          process_4_accommodation: { dirty: false, reason: null },
          process_5_daily_itinerary: { dirty: false, reason: null },
        },
      },
    },
  };
}

export function createStateFile(): object {
  return {
    version: '1.0.0',
    created_at: new Date().toISOString(),
    event_log: [],
    next_actions: ['Search for packages'],
    current_focus: 'process_3_4_packages',
  };
}

export function initProject(options: ProjectInitOptions): { planPath: string; statePath: string } {
  const outputDir = options.outputDir || 'data';
  const plan = createTravelPlan(options);
  const state = createStateFile();

  const planPath = path.join(outputDir, 'travel-plan.json');
  const statePath = path.join(outputDir, 'state.json');

  // Ensure directory exists
  fs.mkdirSync(outputDir, { recursive: true });

  fs.writeFileSync(planPath, JSON.stringify(plan, null, 2));
  fs.writeFileSync(statePath, JSON.stringify(state, null, 2));

  return { planPath, statePath };
}

// CLI execution
if (require.main === module) {
  const args = process.argv.slice(2);

  function getArg(name: string): string | undefined {
    const idx = args.indexOf(`--${name}`);
    return idx >= 0 && idx + 1 < args.length ? args[idx + 1] : undefined;
  }

  const dest = getArg('dest');
  const startDate = getArg('start') || args.find((a) => /^\d{4}-\d{2}-\d{2}$/.test(a));
  const endDate = getArg('end') || args.filter((a) => /^\d{4}-\d{2}-\d{2}$/.test(a))[1];
  const pax = getArg('pax') ? parseInt(getArg('pax')!, 10) : undefined;
  const outputDir = getArg('output');

  if (!dest || !startDate || !endDate) {
    console.log(`
Usage: npx ts-node src/templates/project-init.ts --dest <slug> --start YYYY-MM-DD --end YYYY-MM-DD

Options:
  --dest      Destination slug (required, e.g., tokyo_2026)
  --start     Start date YYYY-MM-DD (required)
  --end       End date YYYY-MM-DD (required)
  --pax       Number of travelers (default: ${DEFAULTS.pax})
  --output    Output directory (default: data)

Available destinations: ${getAvailableDestinations().join(', ')}
`);
    process.exit(1);
  }

  try {
    const result = initProject({
      destination: dest,
      startDate,
      endDate,
      pax,
      outputDir,
    });
    console.log(`✅ Project initialized:`);
    console.log(`   Plan: ${result.planPath}`);
    console.log(`   State: ${result.statePath}`);
  } catch (err) {
    console.error(`❌ Error: ${(err as Error).message}`);
    process.exit(1);
  }
}
