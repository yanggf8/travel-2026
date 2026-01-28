/**
 * @deprecated LEGACY - Do not use for new development.
 *
 * This tool uses ProcessContext with FLAT schema (plan.process_3_transportation)
 * which is incompatible with current NESTED schema
 * (plan.destinations.{slug}.process_3_transportation).
 *
 * Use travel-update.ts CLI instead:
 *   npx ts-node src/cli/travel-update.ts select-offer <id> <date>
 *
 * Or StateManager directly for package-based flow.
 * Kept for reference only.
 */

import {
  ProcessContext,
  ProcessResult,
  FlightCandidate,
  TransportCandidate,
  FlightLeg,
} from './types';
import {
  createContext,
  commitChanges,
  generateId,
  formatResult,
  syncTransportationStatus,
  runCli,
} from './plan-updater';

type FlightDirection = 'outbound' | 'return';
type TransportLegType = 'home_to_airport' | 'airport_to_hotel';

interface AddFlightCandidateInput {
  direction: FlightDirection;
  airline: string;
  flight_number: string;
  departure_airport: string;
  departure_airport_code: string;
  arrival_airport: string;
  arrival_airport_code: string;
  departure_datetime: string;
  arrival_datetime: string;
  duration_minutes: number;
  fare: number;
  fare_class?: string;
  stops?: number;
  layover_airports?: string[];
  baggage_included?: string;
  refundable?: boolean;
  booking_url?: string;
  pros?: string[];
  cons?: string[];
}

interface SelectFlightInput {
  direction: FlightDirection;
  candidate_id: string;
}

interface AddTransportCandidateInput {
  leg: TransportLegType;
  method: string;
  operator?: string;
  route_description: string;
  departure_point: string;
  arrival_point: string;
  duration_minutes: number;
  cost: number;
  departure_time?: string;
  booking_required?: boolean;
  booking_url?: string;
  frequency?: string;
  pros?: string[];
  cons?: string[];
}

interface SelectTransportInput {
  leg: TransportLegType;
  candidate_id: string;
}

export function addFlightCandidate(
  ctx: ProcessContext,
  input: AddFlightCandidateInput
): ProcessResult {
  const result: ProcessResult = {
    success: false,
    processName: 'Transportation',
    action: `Add ${input.direction} flight candidate`,
    changes: [],
    errors: [],
  };

  try {
    const candidate: FlightCandidate = {
      id: generateId(`flight_${input.direction}`),
      direction: input.direction,
      airline: input.airline,
      flight_number: input.flight_number,
      departure_airport: input.departure_airport,
      departure_airport_code: input.departure_airport_code,
      arrival_airport: input.arrival_airport,
      arrival_airport_code: input.arrival_airport_code,
      departure_datetime: input.departure_datetime,
      departure_timezone: null,
      arrival_datetime: input.arrival_datetime,
      arrival_timezone: null,
      duration_minutes: input.duration_minutes,
      stops: input.stops ?? 0,
      layover_airports: input.layover_airports ?? [],
      fare: input.fare,
      fare_class: input.fare_class ?? 'economy',
      baggage_included: input.baggage_included ?? null,
      refundable: input.refundable ?? false,
      booking_url: input.booking_url ?? null,
      pros: input.pros ?? [],
      cons: input.cons ?? [],
    };

    ctx.plan.process_3_transportation.flight.candidates.push(candidate);

    // Update flight sub-status to researched if pending
    if (ctx.plan.process_3_transportation.flight.status === 'pending') {
      ctx.plan.process_3_transportation.flight.status = 'researched';
      result.changes.push('Flight status updated to "researched"');
    }

    // Sync overall process status
    const newStatus = syncTransportationStatus(ctx);
    result.changes.push(`Process status: ${newStatus}`);

    result.changes.push(
      `Added ${input.direction} flight candidate: ${input.airline} ${input.flight_number} (${candidate.id})`
    );
    result.success = true;
  } catch (err) {
    result.errors.push(`Failed to add flight candidate: ${err}`);
  }

  return result;
}

export function selectFlight(
  ctx: ProcessContext,
  input: SelectFlightInput
): ProcessResult {
  const result: ProcessResult = {
    success: false,
    processName: 'Transportation',
    action: `Select ${input.direction} flight`,
    changes: [],
    errors: [],
  };

  try {
    const candidates = ctx.plan.process_3_transportation.flight.candidates;
    const candidate = candidates.find((c) => c.id === input.candidate_id);

    if (!candidate) {
      result.errors.push(`Candidate not found: ${input.candidate_id}`);
      return result;
    }

    // Enforce direction match
    if (candidate.direction !== input.direction) {
      result.errors.push(
        `Direction mismatch: candidate is "${candidate.direction}" but trying to select as "${input.direction}"`
      );
      return result;
    }

    const leg: FlightLeg =
      ctx.plan.process_3_transportation.flight[input.direction];

    leg.airline = candidate.airline;
    leg.flight_number = candidate.flight_number;
    leg.departure_airport = candidate.departure_airport;
    leg.departure_airport_code = candidate.departure_airport_code;
    leg.arrival_airport = candidate.arrival_airport;
    leg.arrival_airport_code = candidate.arrival_airport_code;
    leg.departure_datetime = candidate.departure_datetime;
    leg.arrival_datetime = candidate.arrival_datetime;
    leg.fare = candidate.fare;
    leg.fare_class = candidate.fare_class;
    leg.booking_url = candidate.booking_url;

    // Update flight sub-status to selected
    ctx.plan.process_3_transportation.flight.status = 'selected';

    // Sync overall process status
    const newStatus = syncTransportationStatus(ctx);

    result.changes.push(`Selected ${input.direction} flight: ${candidate.airline} ${candidate.flight_number}`);
    result.changes.push('Flight status updated to "selected"');
    result.changes.push(`Process status: ${newStatus}`);
    result.success = true;
  } catch (err) {
    result.errors.push(`Failed to select flight: ${err}`);
  }

  return result;
}

export function addTransportCandidate(
  ctx: ProcessContext,
  input: AddTransportCandidateInput
): ProcessResult {
  const result: ProcessResult = {
    success: false,
    processName: 'Transportation',
    action: `Add ${input.leg} transport candidate`,
    changes: [],
    errors: [],
  };

  try {
    const candidate: TransportCandidate = {
      id: generateId(`transport_${input.leg}`),
      method: input.method,
      operator: input.operator ?? null,
      route_description: input.route_description,
      departure_point: input.departure_point,
      arrival_point: input.arrival_point,
      departure_time: input.departure_time ?? null,
      duration_minutes: input.duration_minutes,
      cost: input.cost,
      booking_required: input.booking_required ?? false,
      booking_url: input.booking_url ?? null,
      frequency: input.frequency ?? null,
      pros: input.pros ?? [],
      cons: input.cons ?? [],
    };

    const leg = ctx.plan.process_3_transportation[input.leg];
    leg.candidates.push(candidate);

    // Update leg status to researched if pending
    if (leg.status === 'pending') {
      leg.status = 'researched';
      result.changes.push(`${input.leg} status updated to "researched"`);
    }

    // Sync overall process status
    const newStatus = syncTransportationStatus(ctx);
    result.changes.push(`Process status: ${newStatus}`);

    result.changes.push(
      `Added ${input.leg} candidate: ${input.method} - ${input.route_description} (${candidate.id})`
    );
    result.success = true;
  } catch (err) {
    result.errors.push(`Failed to add transport candidate: ${err}`);
  }

  return result;
}

export function selectTransport(
  ctx: ProcessContext,
  input: SelectTransportInput
): ProcessResult {
  const result: ProcessResult = {
    success: false,
    processName: 'Transportation',
    action: `Select ${input.leg} transport`,
    changes: [],
    errors: [],
  };

  try {
    const leg = ctx.plan.process_3_transportation[input.leg];
    const candidate = leg.candidates.find((c) => c.id === input.candidate_id);

    if (!candidate) {
      result.errors.push(`Candidate not found: ${input.candidate_id}`);
      return result;
    }

    leg.method = candidate.method;
    leg.route_description = candidate.route_description;
    leg.departure_point = candidate.departure_point;
    leg.arrival_point = candidate.arrival_point;
    leg.departure_time = candidate.departure_time;
    leg.duration_minutes = candidate.duration_minutes;
    leg.cost = candidate.cost;
    leg.booking_ref = candidate.booking_required ? null : 'N/A';

    // Update leg status to selected
    leg.status = 'selected';

    // Sync overall process status
    const newStatus = syncTransportationStatus(ctx);

    result.changes.push(`Selected ${input.leg}: ${candidate.method}`);
    result.changes.push(`${input.leg} status updated to "selected"`);
    result.changes.push(`Process status: ${newStatus}`);
    result.success = true;
  } catch (err) {
    result.errors.push(`Failed to select transport: ${err}`);
  }

  return result;
}

export function getStatus(ctx: ProcessContext): string {
  const transport = ctx.plan.process_3_transportation;
  const lines: string[] = [];

  lines.push('=== Transportation Status ===');
  lines.push(`Overall: ${transport.status}`);
  lines.push('');

  // Flight status
  lines.push(`Flight: ${transport.flight.status}`);
  const outboundCandidates = transport.flight.candidates.filter(c => c.direction === 'outbound').length;
  const returnCandidates = transport.flight.candidates.filter(c => c.direction === 'return').length;
  lines.push(`  Candidates: ${transport.flight.candidates.length} (outbound: ${outboundCandidates}, return: ${returnCandidates})`);
  if (transport.flight.outbound.airline) {
    lines.push(`  Outbound: ${transport.flight.outbound.airline} ${transport.flight.outbound.flight_number}`);
  }
  if (transport.flight.return.airline) {
    lines.push(`  Return: ${transport.flight.return.airline} ${transport.flight.return.flight_number}`);
  }
  lines.push('');

  // Ground transport status
  lines.push(`Home→Airport: ${transport.home_to_airport.status}`);
  lines.push(`  Candidates: ${transport.home_to_airport.candidates.length}`);
  if (transport.home_to_airport.method) {
    lines.push(`  Selected: ${transport.home_to_airport.method}`);
  }
  lines.push('');

  lines.push(`Airport→Hotel: ${transport.airport_to_hotel.status}`);
  lines.push(`  Candidates: ${transport.airport_to_hotel.candidates.length}`);
  if (transport.airport_to_hotel.method) {
    lines.push(`  Selected: ${transport.airport_to_hotel.method}`);
  }

  return lines.join('\n');
}

// CLI interface
function printUsage(): void {
  console.log('Transportation Process Tool');
  console.log('');
  console.log('Usage: ts-node src/process/transportation.ts <command> [options]');
  console.log('');
  console.log('Commands:');
  console.log('  status                    Show transportation status');
  console.log('  add-flight <json>         Add flight candidate (JSON input with direction)');
  console.log('  select-flight <json>      Select flight (JSON: {direction, candidate_id})');
  console.log('  add-transport <json>      Add transport candidate (JSON input)');
  console.log('  select-transport <json>   Select transport (JSON: {leg, candidate_id})');
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

  // Parse --file option
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

    case 'add-flight': {
      if (!args[1]) {
        console.error('Error: add-flight requires JSON input');
        printUsage();
        process.exit(1);
      }
      const input = JSON.parse(args[1]) as AddFlightCandidateInput;
      const result = addFlightCandidate(ctx, input);
      console.log(formatResult(result));
      if (result.success && !dryRun) {
        commitChanges(ctx);
        console.log('\nChanges saved.');
      }
      break;
    }

    case 'select-flight': {
      if (!args[1]) {
        console.error('Error: select-flight requires JSON input');
        printUsage();
        process.exit(1);
      }
      const input = JSON.parse(args[1]) as SelectFlightInput;
      const result = selectFlight(ctx, input);
      console.log(formatResult(result));
      if (result.success && !dryRun) {
        commitChanges(ctx);
        console.log('\nChanges saved.');
      }
      break;
    }

    case 'add-transport': {
      if (!args[1]) {
        console.error('Error: add-transport requires JSON input');
        printUsage();
        process.exit(1);
      }
      const input = JSON.parse(args[1]) as AddTransportCandidateInput;
      const result = addTransportCandidate(ctx, input);
      console.log(formatResult(result));
      if (result.success && !dryRun) {
        commitChanges(ctx);
        console.log('\nChanges saved.');
      }
      break;
    }

    case 'select-transport': {
      if (!args[1]) {
        console.error('Error: select-transport requires JSON input');
        printUsage();
        process.exit(1);
      }
      const input = JSON.parse(args[1]) as SelectTransportInput;
      const result = selectTransport(ctx, input);
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
