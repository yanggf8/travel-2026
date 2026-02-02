/**
 * @deprecated LEGACY - Do not use for new development.
 *
 * This tool uses ProcessContext with FLAT schema (plan.process_4_accommodation)
 * which is incompatible with current NESTED schema
 * (plan.destinations.{slug}.process_4_accommodation).
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
  ZoneCandidate,
  HotelCandidate,
} from './types';
import {
  createContext,
  commitChanges,
  generateId,
  formatResult,
  syncAccommodationStatus,
  runCli,
} from './plan-updater';

interface AddZoneCandidateInput {
  name: string;
  description: string;
  pros?: string[];
  cons?: string[];
  main_stations?: string[];
  hotel_price_range?: { min: number | null; max: number | null };
  distance_to_center?: string;
  vibe?: string;
}

interface SelectZoneInput {
  candidate_id: string;
  criteria?: string;
}

interface AddHotelCandidateInput {
  name: string;
  address: string;
  area: string;
  rating: number;
  review_count: number;
  price_per_night: number;
  total_price: number;
  amenities?: string[];
  room_types?: string[];
  distance_to_station?: string;
  nearest_station?: string;
  booking_url?: string;
  cancellation_policy?: string;
  pros?: string[];
  cons?: string[];
}

interface SelectHotelInput {
  candidate_id: string;
}

interface BookHotelInput {
  confirmation_number: string;
  check_in_date: string;
  check_in_time?: string;
  check_out_date: string;
  check_out_time?: string;
  room_type: string;
  total_price: number;
  price_per_night: number;
  cancellation_policy?: string;
  cancellation_deadline?: string;
  booking_url?: string;
}

export function addZoneCandidate(
  ctx: ProcessContext,
  input: AddZoneCandidateInput
): ProcessResult {
  const result: ProcessResult = {
    success: false,
    processName: 'Accommodation',
    action: 'Add zone candidate',
    changes: [],
    errors: [],
  };

  try {
    const candidate: ZoneCandidate = {
      id: generateId('zone'),
      name: input.name,
      description: input.description,
      pros: input.pros ?? [],
      cons: input.cons ?? [],
      main_stations: input.main_stations ?? [],
      hotel_price_range: input.hotel_price_range ?? { min: null, max: null },
      distance_to_center: input.distance_to_center ?? null,
      vibe: input.vibe ?? null,
    };

    ctx.plan.process_4_accommodation.location_zone.candidates.push(candidate);

    // Update zone sub-status to researched if pending
    if (ctx.plan.process_4_accommodation.location_zone.status === 'pending') {
      ctx.plan.process_4_accommodation.location_zone.status = 'researched';
      result.changes.push('Zone status updated to "researched"');
    }

    // Sync overall process status
    const newStatus = syncAccommodationStatus(ctx);
    result.changes.push(`Process status: ${newStatus}`);

    result.changes.push(`Added zone candidate: ${input.name} (${candidate.id})`);
    result.success = true;
  } catch (err) {
    result.errors.push(`Failed to add zone candidate: ${err}`);
  }

  return result;
}

export function selectZone(
  ctx: ProcessContext,
  input: SelectZoneInput
): ProcessResult {
  const result: ProcessResult = {
    success: false,
    processName: 'Accommodation',
    action: 'Select zone',
    changes: [],
    errors: [],
  };

  try {
    const candidates = ctx.plan.process_4_accommodation.location_zone.candidates;
    const candidate = candidates.find((c) => c.id === input.candidate_id);

    if (!candidate) {
      result.errors.push(`Zone candidate not found: ${input.candidate_id}`);
      return result;
    }

    ctx.plan.process_4_accommodation.location_zone.selected_area = candidate.name;
    ctx.plan.process_4_accommodation.location_zone.selection_criteria =
      input.criteria ?? null;
    ctx.plan.process_4_accommodation.location_zone.status = 'selected';

    // Sync overall process status
    const newStatus = syncAccommodationStatus(ctx);

    result.changes.push(`Selected zone: ${candidate.name}`);
    result.changes.push('Zone status updated to "selected"');
    result.changes.push(`Process status: ${newStatus}`);
    result.success = true;
  } catch (err) {
    result.errors.push(`Failed to select zone: ${err}`);
  }

  return result;
}

export function addHotelCandidate(
  ctx: ProcessContext,
  input: AddHotelCandidateInput
): ProcessResult {
  const result: ProcessResult = {
    success: false,
    processName: 'Accommodation',
    action: 'Add hotel candidate',
    changes: [],
    errors: [],
  };

  try {
    const candidate: HotelCandidate = {
      id: generateId('hotel'),
      name: input.name,
      address: input.address,
      area: input.area,
      rating: input.rating,
      review_count: input.review_count,
      price_per_night: input.price_per_night,
      total_price: input.total_price,
      amenities: input.amenities ?? [],
      room_types: input.room_types ?? [],
      distance_to_station: input.distance_to_station ?? null,
      nearest_station: input.nearest_station ?? null,
      booking_url: input.booking_url ?? null,
      cancellation_policy: input.cancellation_policy ?? null,
      pros: input.pros ?? [],
      cons: input.cons ?? [],
    };

    ctx.plan.process_4_accommodation.hotel.candidates.push(candidate);

    // Update hotel sub-status to researched if pending
    if (ctx.plan.process_4_accommodation.hotel.status === 'pending') {
      ctx.plan.process_4_accommodation.hotel.status = 'researched';
      result.changes.push('Hotel status updated to "researched"');
    }

    // Sync overall process status
    const newStatus = syncAccommodationStatus(ctx);
    result.changes.push(`Process status: ${newStatus}`);

    result.changes.push(
      `Added hotel candidate: ${input.name} - ¥${input.price_per_night}/night (${candidate.id})`
    );
    result.success = true;
  } catch (err) {
    result.errors.push(`Failed to add hotel candidate: ${err}`);
  }

  return result;
}

export function selectHotel(
  ctx: ProcessContext,
  input: SelectHotelInput
): ProcessResult {
  const result: ProcessResult = {
    success: false,
    processName: 'Accommodation',
    action: 'Select hotel',
    changes: [],
    errors: [],
  };

  try {
    const candidates = ctx.plan.process_4_accommodation.hotel.candidates;
    const candidate = candidates.find((c) => c.id === input.candidate_id);

    if (!candidate) {
      result.errors.push(`Hotel candidate not found: ${input.candidate_id}`);
      return result;
    }

    ctx.plan.process_4_accommodation.hotel.selected_hotel = candidate.name;
    ctx.plan.process_4_accommodation.hotel.status = 'selected';

    // Sync overall process status
    const newStatus = syncAccommodationStatus(ctx);

    result.changes.push(`Selected hotel: ${candidate.name}`);
    result.changes.push('Hotel status updated to "selected"');
    result.changes.push(`Process status: ${newStatus}`);
    result.success = true;
  } catch (err) {
    result.errors.push(`Failed to select hotel: ${err}`);
  }

  return result;
}

export function bookHotel(
  ctx: ProcessContext,
  input: BookHotelInput
): ProcessResult {
  const result: ProcessResult = {
    success: false,
    processName: 'Accommodation',
    action: 'Book hotel',
    changes: [],
    errors: [],
  };

  try {
    if (!ctx.plan.process_4_accommodation.hotel.selected_hotel) {
      result.errors.push('No hotel selected. Select a hotel first.');
      return result;
    }

    const booking = ctx.plan.process_4_accommodation.hotel.booking;
    booking.confirmation_number = input.confirmation_number;
    booking.check_in_date = input.check_in_date;
    booking.check_in_time = input.check_in_time ?? null;
    booking.check_out_date = input.check_out_date;
    booking.check_out_time = input.check_out_time ?? null;
    booking.room_type = input.room_type;
    booking.total_price = input.total_price;
    booking.price_per_night = input.price_per_night;
    booking.cancellation_policy = input.cancellation_policy ?? null;
    booking.cancellation_deadline = input.cancellation_deadline ?? null;
    booking.booking_url = input.booking_url ?? null;

    ctx.plan.process_4_accommodation.hotel.status = 'booked';

    // Sync overall process status
    const newStatus = syncAccommodationStatus(ctx);

    result.changes.push(`Booked hotel: ${ctx.plan.process_4_accommodation.hotel.selected_hotel}`);
    result.changes.push(`Confirmation: ${input.confirmation_number}`);
    result.changes.push(`Check-in: ${input.check_in_date}, Check-out: ${input.check_out_date}`);
    result.changes.push('Hotel status updated to "booked"');
    result.changes.push(`Process status: ${newStatus}`);
    result.success = true;
  } catch (err) {
    result.errors.push(`Failed to book hotel: ${err}`);
  }

  return result;
}

export function getStatus(ctx: ProcessContext): string {
  const accom = ctx.plan.process_4_accommodation;
  const lines: string[] = [];

  lines.push('=== Accommodation Status ===');
  lines.push(`Overall: ${accom.status}`);
  lines.push('');

  // Zone status
  lines.push(`Zone: ${accom.location_zone.status}`);
  lines.push(`  Candidates: ${accom.location_zone.candidates.length}`);
  if (accom.location_zone.selected_area) {
    lines.push(`  Selected: ${accom.location_zone.selected_area}`);
    if (accom.location_zone.selection_criteria) {
      lines.push(`  Criteria: ${accom.location_zone.selection_criteria}`);
    }
  }
  lines.push('');

  // Hotel status
  lines.push(`Hotel: ${accom.hotel.status}`);
  lines.push(`  Candidates: ${accom.hotel.candidates.length}`);
  if (accom.hotel.selected_hotel) {
    lines.push(`  Selected: ${accom.hotel.selected_hotel}`);
  }
  if (accom.hotel.booking.confirmation_number) {
    lines.push(`  Booking: ${accom.hotel.booking.confirmation_number}`);
    lines.push(`  Check-in: ${accom.hotel.booking.check_in_date}`);
    lines.push(`  Check-out: ${accom.hotel.booking.check_out_date}`);
  }

  return lines.join('\n');
}

// CLI interface
function printUsage(): void {
  console.log('Accommodation Process Tool');
  console.log('');
  console.log('Usage: ts-node src/process/accommodation.ts <command> [options]');
  console.log('');
  console.log('Commands:');
  console.log('  status                    Show accommodation status');
  console.log('  add-zone <json>           Add zone candidate (JSON input)');
  console.log('  select-zone <json>        Select zone (JSON: {candidate_id, criteria?})');
  console.log('  add-hotel <json>          Add hotel candidate (JSON input)');
  console.log('  select-hotel <json>       Select hotel (JSON: {candidate_id})');
  console.log('  book-hotel <json>         Book hotel (JSON with booking details)');
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

    case 'add-zone': {
      if (!args[1]) {
        console.error('Error: add-zone requires JSON input');
        printUsage();
        process.exit(1);
      }
      const input = JSON.parse(args[1]) as AddZoneCandidateInput;
      const result = addZoneCandidate(ctx, input);
      console.log(formatResult(result));
      if (result.success && !dryRun) {
        commitChanges(ctx);
        console.log('\nChanges saved.');
      }
      break;
    }

    case 'select-zone': {
      if (!args[1]) {
        console.error('Error: select-zone requires JSON input');
        printUsage();
        process.exit(1);
      }
      const input = JSON.parse(args[1]) as SelectZoneInput;
      const result = selectZone(ctx, input);
      console.log(formatResult(result));
      if (result.success && !dryRun) {
        commitChanges(ctx);
        console.log('\nChanges saved.');
      }
      break;
    }

    case 'add-hotel': {
      if (!args[1]) {
        console.error('Error: add-hotel requires JSON input');
        printUsage();
        process.exit(1);
      }
      const input = JSON.parse(args[1]) as AddHotelCandidateInput;
      const result = addHotelCandidate(ctx, input);
      console.log(formatResult(result));
      if (result.success && !dryRun) {
        commitChanges(ctx);
        console.log('\nChanges saved.');
      }
      break;
    }

    case 'select-hotel': {
      if (!args[1]) {
        console.error('Error: select-hotel requires JSON input');
        printUsage();
        process.exit(1);
      }
      const input = JSON.parse(args[1]) as SelectHotelInput;
      const result = selectHotel(ctx, input);
      console.log(formatResult(result));
      if (result.success && !dryRun) {
        commitChanges(ctx);
        console.log('\nChanges saved.');
      }
      break;
    }

    case 'book-hotel': {
      if (!args[1]) {
        console.error('Error: book-hotel requires JSON input');
        printUsage();
        process.exit(1);
      }
      const input = JSON.parse(args[1]) as BookHotelInput;
      const result = bookHotel(ctx, input);
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
