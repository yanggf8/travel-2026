#!/usr/bin/env node
/**
 * /p3p4-packages end-to-end test (offline)
 *
 * Simulates a user selecting a package offer, marks the package process dirty,
 * then runs the cascade runner to populate:
 * - process_3_transportation.flight
 * - process_4_accommodation.hotel
 *
 * Usage:
 *   npx ts-node src/cli/p3p4-test.ts
 *   npx ts-node src/cli/p3p4-test.ts --offer-id besttour_TYO05MM260211AM
 *   npx ts-node src/cli/p3p4-test.ts --input data/trips/tokyo-2026/travel-plan.json --output data/travel-plan.p3p4-test.json
 */

import { writeFileSync } from 'fs';
import { computePlan, applyPlan, loadPlan } from '../cascade/runner';

type Args = {
  input: string;
  output: string;
  offerId?: string;
  dryRun: boolean;
};

function parseArgs(argv: string[]): Args {
  const args: Args = {
    input: 'data/trips/tokyo-2026/travel-plan.json',
    output: 'data/travel-plan.p3p4-test.json',
    offerId: undefined,
    dryRun: false,
  };

  for (let i = 2; i < argv.length; i++) {
    const arg = argv[i];
    switch (arg) {
      case '--input':
      case '-i':
        args.input = argv[++i];
        break;
      case '--output':
      case '-o':
        args.output = argv[++i];
        break;
      case '--offer-id':
        args.offerId = argv[++i];
        break;
      case '--dry-run':
        args.dryRun = true;
        break;
      case '--help':
      case '-h':
        console.log(`
/p3p4-packages E2E test (offline)

OPTIONS:
  -i, --input <path>     Input travel-plan.json (default: data/trips/tokyo-2026/travel-plan.json)
  -o, --output <path>    Output file (default: data/travel-plan.p3p4-test.json)
  --offer-id <id>        Offer id to select (default: cheapest offer)
  --dry-run              Print cascade plan only; do not write output
  -h, --help             Show help
`);
        process.exit(0);
      default:
        if (arg.startsWith('-')) {
          console.error(`Unknown option: ${arg}`);
          process.exit(1);
        }
        break;
    }
  }

  return args;
}

function nowIso(): string {
  return new Date().toISOString();
}

function main(): void {
  const args = parseArgs(process.argv);
  const plan = loadPlan(args.input);

  const active = plan.active_destination;
  const dest = plan.destinations[active];
  if (!dest) {
    throw new Error(`Active destination not found: ${active}`);
  }

  const p34 = dest.process_3_4_packages as any;
  if (!p34?.results?.offers || !Array.isArray(p34.results.offers) || p34.results.offers.length === 0) {
    throw new Error(`No offers found at destinations.${active}.process_3_4_packages.results.offers`);
  }

  const offers = p34.results.offers as any[];
  const selected =
    args.offerId ? offers.find(o => o?.id === args.offerId) : offers.slice().sort((a, b) => a.price_per_person - b.price_per_person)[0];

  if (!selected) {
    throw new Error(`Offer not found: ${args.offerId}`);
  }

  // Keep destination-change trigger quiet; this test focuses on package selection cascade.
  plan.cascade_state.global.active_destination_last = active;

  // Reset P3/P4 targets so population effect is visible.
  const p3 = dest.process_3_transportation as any;
  if (p3) {
    p3.status = 'pending';
    p3.updated_at = nowIso();
    p3.source = null;
    if (!p3.flight || typeof p3.flight !== 'object') p3.flight = {};
    p3.flight.status = 'pending';
    p3.flight.candidates = [];
    p3.flight.outbound = {};
    p3.flight.return = {};
  }

  const p4 = dest.process_4_accommodation as any;
  if (p4) {
    p4.status = 'pending';
    p4.updated_at = nowIso();
    p4.source = null;
    if (!p4.hotel || typeof p4.hotel !== 'object') p4.hotel = {};
    p4.hotel.status = 'pending';
    p4.hotel.candidates = [];
    p4.hotel.selected_hotel = null;
  }

  // Simulate user selection in /p3p4-packages.
  p34.status = 'selected';
  p34.updated_at = nowIso();
  p34.selected_offer_id = selected.id;
  p34.results.chosen_offer = selected;

  // Mark the source process dirty so `process_3_4_packages_selected` fires.
  if (!plan.cascade_state.destinations[active]) plan.cascade_state.destinations[active] = {};
  plan.cascade_state.destinations[active].process_3_4_packages = {
    dirty: true,
    last_changed: nowIso(),
  };

  const cascadePlan = computePlan(plan);
  console.log(JSON.stringify(cascadePlan, null, 2));

  if (args.dryRun) return;

  const updated = applyPlan(plan, cascadePlan);
  writeFileSync(args.output, JSON.stringify(updated, null, 2), 'utf-8');

  const updatedDest = updated.destinations[active] as any;
  const p3Source = updatedDest?.process_3_transportation?.source;
  const p4Source = updatedDest?.process_4_accommodation?.source;

  console.log(`\nWrote: ${args.output}`);
  console.log(`Selected offer: ${selected.id}`);
  console.log(`P3 source: ${p3Source ?? '(null)'}`);
  console.log(`P4 source: ${p4Source ?? '(null)'}`);
}

main();

