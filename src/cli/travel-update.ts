#!/usr/bin/env npx ts-node
/**
 * Travel Update CLI
 *
 * Quick command-line interface for updating travel plan state.
 * Supports date changes, offer updates, and selections without editing JSON.
 *
 * Usage:
 *   npx ts-node src/cli/travel-update.ts <command> [options]
 *
 * Commands are registered via the modules imported below; see commands/help.ts
 * for the full HELP text. Each command is a CommandHandler in commands/.
 */

import { StateManager } from '../state/state-manager';
import { getCommandHandler } from './commands/registry';
import { HELP } from './commands/help';
import './commands/help';
import './commands/status';
import './commands/itinerary';
import './commands/transport';
import './commands/bookings';
import './commands/set-dates';
import './commands/offers';
import './commands/set-flight';
import './commands/set-hotel';
import './commands/set-airport-transfer';
import './commands/activity';
import './commands/session';
import './commands/route';
import './commands/day';
import './commands/weather';
import './commands/ops';
import './commands/scrape-package';
import './commands/scaffold-itinerary';
import './commands/mark-booked';
import './commands/populate-itinerary';
import './commands/validate';
import './commands/search-compare';
import './commands/view-prices';
import './commands/turso';
import './commands/plans';
import './commands/stage0';
import { makeParsedArgs } from './shared/args';
import { listPlans, resolvePlanFromSummaries } from './shared/plan-resolver';

async function main(): Promise<void> {
  const args = process.argv.slice(2);
  const command = args[0];

  if (!command || command === 'help' || command === '--help' || command === '-h') {
    console.log(HELP);
    process.exit(0);
  }

  const dryRun = args.includes('--dry-run');
  const verbose = args.includes('--verbose');
  const parsedArgs = makeParsedArgs(args);

  const handler = getCommandHandler(command);
  if (!handler) {
    console.error(`Unknown command: ${command}`);
    console.log(HELP);
    process.exit(1);
  }

  const explicitPlanId = parsedArgs.optionValue('--plan-id');
  const envPlanId = process.env.TRAVEL_PLAN_ID;
  const planOpt = parsedArgs.optionValue('--plan');
  const stateOpt = parsedArgs.optionValue('--state');

  if (planOpt && !explicitPlanId && !envPlanId) {
    console.warn('⚠️  --plan is deprecated. Use --plan-id or set TRAVEL_PLAN_ID env var instead.');
  }
  if ((explicitPlanId || envPlanId) && (planOpt || stateOpt)) {
    console.error('Error: --plan-id cannot be combined with --plan or --state');
    process.exit(1);
  }

  if (handler.requiresState === false) {
    await handler.execute({
      sm: undefined as any,
      args: parsedArgs,
      dryRun,
      verbose,
      planId: explicitPlanId || envPlanId,
    });
    return;
  }

  // Pre-load destination config from Turso DB so all sync loader APIs work
  const { loadDestinationConfigFromDb } = await import('../config/loader');
  await loadDestinationConfigFromDb();

  const plans = await listPlans();
  const resolvedPlan = resolvePlanFromSummaries({
    explicitPlanId,
    envPlanId,
    planPath: planOpt,
    date: parsedArgs.optionValue('--travel-date'),
    rangeStart: parsedArgs.optionValue('--travel-start'),
    rangeEnd: parsedArgs.optionValue('--travel-end'),
  }, plans);

  if (resolvedPlan.note) {
    console.error(`[plan] ${resolvedPlan.note}`);
  }
  if (verbose || !['explicit', 'env', 'plan-path'].includes(resolvedPlan.source)) {
    console.error(`[plan] Using ${resolvedPlan.planId} (${resolvedPlan.source})`);
  }

  const sm = await StateManager.createFromPlanId(resolvedPlan.planId);
  await handler.execute({ sm, args: parsedArgs, dryRun, verbose, planId: resolvedPlan.planId });
}

main().catch((err) => {
  console.error(err instanceof Error ? err.message : String(err));
  process.exit(1);
});
