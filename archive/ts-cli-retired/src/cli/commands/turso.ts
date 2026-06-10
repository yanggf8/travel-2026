import type { CommandHandler, CliContext } from '../shared/types';
import { registerCommand } from './registry';
import { toDestSlug } from '../../utils/plan-id';
import * as fs from 'fs';
import * as path from 'path';

// ── import-offers ────────────────────────────────────────────────────

const importOffersCommand: CommandHandler = {
  names: ['import-offers'],
  description: 'Import scraped offers from JSON files into Turso DB.',
  usage: 'import-offers [--dest <slug>] [--dir <path>] [--files <csv>] [--start <date>] [--end <date>] [--pax N] [--note <text>] [--dry-run]',
  async execute(ctx: CliContext): Promise<void> {
    const { sm, args, dryRun } = ctx;
    const destOpt = args.optionValue('--dest');
    const dirOpt = args.optionValue('--dir');
    const filesOpt = args.optionValue('--files');
    const startOpt = args.optionValue('--start');
    const endOpt = args.optionValue('--end');
    const paxOpt = args.optionValue('--pax');
    const noteOpt = args.optionValue('--note');

    const destination = sm.resolveDestination(destOpt);
    let filePaths: string[];
    if (filesOpt) {
      filePaths = filesOpt.split(',').map(s => s.trim()).filter(Boolean);
    } else {
      const dir = dirOpt || 'scrapes';
      filePaths = fs.existsSync(dir)
        ? fs.readdirSync(dir)
          .filter(f => f.endsWith('.json') && !f.includes('schema') && !f.includes('destinations') && !f.includes('ota-sources'))
          .map(f => path.join(dir, f))
        : [];
    }
    if (filePaths.length === 0) {
      console.error('No JSON files found. Use --dir <path> or --files <csv>.');
      process.exit(1);
    }
    console.log(`Importing offers for ${destination} from ${filePaths.length} file(s)${dryRun ? ' (dry-run)' : ''}...`);
    const counts = await sm.importScrapedOffers(destination, filePaths, {
      start: startOpt,
      end: endOpt,
      pax: paxOpt ? parseInt(paxOpt, 10) : 2,
      dryRun,
      note: noteOpt,
    });
    if (Object.keys(counts).length === 0) {
      console.log('No offers imported (no matching files or all offers filtered out).');
    } else {
      for (const [src, count] of Object.entries(counts)) {
        console.log(`  ${src}: ${count} offer(s)`);
      }
      if (!dryRun) {
        console.log(`Saved to Turso (plan_offers).`);
      }
    }
  },
};

registerCommand(importOffersCommand);

// ── query-offers ─────────────────────────────────────────────────────

const queryOffersCommand: CommandHandler = {
  names: ['query-offers'],
  description: 'Query offers from Turso DB with filters.',
  usage: 'query-offers [--plan-id <id>] [--dest <slug>] [--region <name>] [--start <date>] [--end <date>] [--source <id>] [--types <type>] [--max-price N] [--fresh-hours N] [--max N] [--json]',
  requiresState: false,
  async execute(ctx: CliContext): Promise<void> {
    const { sm, args } = ctx;
    const planIdOpt = args.optionValue('--plan-id') || process.env.TRAVEL_PLAN_ID || ctx.planId;
    const destOpt = args.optionValue('--dest');
    const sourceOpt = args.optionValue('--source');
    const startOpt = args.optionValue('--start');
    const endOpt = args.optionValue('--end');
    const typesOpt = args.optionValue('--types');
    const maxPriceOpt = args.optionValue('--max-price');
    const freshHoursOpt = args.optionValue('--fresh-hours');
    const maxOpt = args.optionValue('--max');
    const jsonOpt = args.hasFlag('--json');

    const { queryOffers, printTursoOfferTable } = await import('../../services/turso-service');
    const results = await queryOffers({
      planId: planIdOpt,
      destination: destOpt,
      region: args.optionValue('--region'),
      start: startOpt,
      end: endOpt,
      sources: sourceOpt?.split(','),
      type: typesOpt as 'package' | 'flight' | 'hotel' | undefined,
      maxPrice: maxPriceOpt ? parseInt(maxPriceOpt, 10) : undefined,
      freshHours: freshHoursOpt ? parseInt(freshHoursOpt, 10) : undefined,
      limit: maxOpt ? parseInt(maxOpt, 10) : undefined,
    });
    if (jsonOpt) {
      console.log(JSON.stringify(results, null, 2));
    } else {
      printTursoOfferTable(results);
    }
  },
};

registerCommand(queryOffersCommand);

// ── check-freshness ──────────────────────────────────────────────────

const checkFreshnessCommand: CommandHandler = {
  names: ['check-freshness'],
  description: 'Check data freshness for an OTA source in Turso DB.',
  usage: 'check-freshness --source <id> [--region <name>] [--start <date>] [--end <date>] [--max-age N] [--plan-id <id>] [--dest <slug>]',
  requiresState: false,
  async execute(ctx: CliContext): Promise<void> {
    const { args } = ctx;
    const sourceOpt = args.optionValue('--source');
    const planIdOpt = args.optionValue('--plan-id') || process.env.TRAVEL_PLAN_ID || ctx.planId;
    const destOpt = args.optionValue('--dest');
    const startOpt = args.optionValue('--start');
    const endOpt = args.optionValue('--end');
    const maxAgeOpt = args.optionValue('--max-age');

    const source = sourceOpt;
    if (!source) {
      console.error('Error: check-freshness requires --source <id>');
      console.error('Example: check-freshness --source besttour --region kansai');
      process.exit(1);
    }
    const { checkFreshness } = await import('../../services/turso-service');
    const result = await checkFreshness(source, {
      region: args.optionValue('--region'),
      start: startOpt,
      end: endOpt,
      maxAgeHours: maxAgeOpt ? parseInt(maxAgeOpt, 10) : 24,
      planId: planIdOpt,
      destination: destOpt,
    });
    console.log(`Source:  ${source}`);
    if (result.region) console.log(`Region:  ${result.region}`);
    console.log(`Result:  ${result.recommendation}`);
    if (result.ageHours !== null) console.log(`  Age:     ${result.ageHours.toFixed(1)}h`);
    console.log(`  Offers:  ${result.offerCount}`);
  },
};

registerCommand(checkFreshnessCommand);

// ── sync-bookings ────────────────────────────────────────────────────

const syncBookingsCommand: CommandHandler = {
  names: ['sync-bookings'],
  description: 'Sync bookings from plan JSON to Turso DB.',
  usage: 'sync-bookings [--plan-id <id>] [--trip-id <id>] [--dry-run]',
  async execute(ctx: CliContext): Promise<void> {
    const { dryRun } = ctx;
    const planIdOpt = ctx.args.optionValue('--plan-id') || process.env.TRAVEL_PLAN_ID || ctx.planId;
    const tripIdOpt = ctx.args.optionValue('--trip-id');

    const { syncBookingsFromPlanJson } = await import('../../services/turso-service');
    const { TursoRepository } = await import('../../state/turso-repository');
    const effectivePlanId = planIdOpt;
    if (!effectivePlanId) throw new Error('No plan resolved. Use --plan-id <id>.');
    console.log(`Syncing bookings from plan "${effectivePlanId}"...`);
    const repo = await TursoRepository.create(effectivePlanId);
    const plan = repo.getPlan() as unknown as Record<string, unknown>;
    const effectiveTripId = tripIdOpt || toDestSlug(effectivePlanId);
    const syncResult = await syncBookingsFromPlanJson(plan, effectiveTripId, {
      dryRun: dryRun,
    });
    if (syncResult.warnings.length > 0) {
      console.warn('Warnings:');
      for (const w of syncResult.warnings) console.warn(`  - ${w}`);
    }
    console.log(`${dryRun ? 'Would sync' : 'Synced'} ${syncResult.synced} bookings to Turso.`);
  },
};

registerCommand(syncBookingsCommand);

// ── query-bookings ───────────────────────────────────────────────────

const queryBookingsCommand: CommandHandler = {
  names: ['query-bookings'],
  description: 'Query bookings from Turso DB with filters.',
  usage: 'query-bookings [--trip-id <id>] [--dest <slug>] [--category <type>] [--status <status>] [--max N] [--json]',
  requiresState: false,
  async execute(ctx: CliContext): Promise<void> {
    const { args } = ctx;
    const tripIdOpt = args.optionValue('--trip-id');
    const destOpt = args.optionValue('--dest');
    const categoryOpt = args.optionValue('--category');
    const statusFilterOpt = args.optionValue('--status');
    const maxOpt = args.optionValue('--max');
    const jsonOpt = args.hasFlag('--json');

    const { queryBookings, printBookingsTable } = await import('../../services/turso-service');
    const results = await queryBookings({
      tripId: tripIdOpt,
      destination: destOpt,
      category: categoryOpt as 'package' | 'transfer' | 'activity' | undefined,
      status: statusFilterOpt,
      limit: maxOpt ? parseInt(maxOpt, 10) : undefined,
    });
    if (jsonOpt) {
      console.log(JSON.stringify(results, null, 2));
    } else {
      printBookingsTable(results);
    }
  },
};

registerCommand(queryBookingsCommand);

// ── check-booking-integrity ──────────────────────────────────────────

const checkBookingIntegrityCommand: CommandHandler = {
  names: ['check-booking-integrity'],
  description: 'Check booking integrity between plan state and Turso DB.',
  usage: 'check-booking-integrity [--plan-id <id>] [--trip-id <id>]',
  async execute(ctx: CliContext): Promise<void> {
    const { sm, args } = ctx;
    const planIdOpt = args.optionValue('--plan-id') || process.env.TRAVEL_PLAN_ID || ctx.planId;
    const tripIdOpt = args.optionValue('--trip-id');

    const { checkBookingIntegrity } = await import('../../services/turso-service');
    const effectivePlanId = planIdOpt;
    if (!effectivePlanId) throw new Error('No plan resolved. Use --plan-id <id>.');
    const effectiveTripId = tripIdOpt || toDestSlug(effectivePlanId);
    const plan = sm.getPlan() as unknown as Record<string, unknown>;
    console.log(`Checking booking integrity (plan "${effectivePlanId}" vs Turso DB)...`);
    const integrity = await checkBookingIntegrity(plan, effectiveTripId);
    console.log(`\nResults:`);
    console.log(`  Matches:    ${integrity.matches}`);
    console.log(`  Mismatches: ${integrity.mismatches.length}`);
    console.log(`  Plan-only:  ${integrity.planOnly.length}`);
    console.log(`  DB-only:    ${integrity.dbOnly.length}`);
    if (integrity.mismatches.length > 0) {
      console.log('\nMismatches:');
      for (const m of integrity.mismatches) console.log(`  - ${m}`);
    }
    if (integrity.planOnly.length > 0) {
      console.log('\nPlan-only (not in DB):');
      for (const p of integrity.planOnly) console.log(`  - ${p}`);
    }
    if (integrity.dbOnly.length > 0) {
      console.log('\nDB-only (not in plan):');
      for (const d of integrity.dbOnly) console.log(`  - ${d}`);
    }
  },
};

registerCommand(checkBookingIntegrityCommand);
