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
 * Commands:
 *   set-dates <start> <end> [reason]     Set travel dates (triggers cascade)
 *   update-offer <offer-id> <date> <availability> [price] [seats]
 *   select-offer <offer-id> <date>       Select an offer for booking
 *   status                               Show current plan status
 * 
 * Examples:
 *   npx ts-node src/cli/travel-update.ts set-dates 2026-02-13 2026-02-17 "Agent offered Feb 13"
 *   npx ts-node src/cli/travel-update.ts update-offer besttour_TYO05MM260211AM 2026-02-13 available 27888 2
 *   npx ts-node src/cli/travel-update.ts select-offer besttour_TYO05MM260211AM 2026-02-13
 *   npx ts-node src/cli/travel-update.ts status
 */

import { StateManager } from '../state/state-manager';

const HELP = `
Travel Update CLI - Quick updates to travel plan

Usage:
  npx ts-node src/cli/travel-update.ts <command> [options]

Commands:
  set-dates <start> <end> [reason]
    Set travel dates. Triggers cascade to invalidate dependent processes.
    Example: set-dates 2026-02-13 2026-02-17 "Agent offered Feb 13"

  update-offer <offer-id> <date> <availability> [price] [seats] [source]
    Update offer availability for a specific date.
    availability: available | sold_out | limited
    Example: update-offer besttour_TYO05MM260211AM 2026-02-13 available 27888 2 agent

  select-offer <offer-id> <date> [--no-populate]
    Select an offer for booking. Populates P3/P4 from offer by default.
    Example: select-offer besttour_TYO05MM260211AM 2026-02-13

  status
    Show current plan status summary.

  help
    Show this help message.

Options:
  --dry-run    Show what would be changed without saving
  --verbose    Show detailed output
`;

function formatDate(dateStr: string): string {
  const d = new Date(dateStr);
  return d.toLocaleDateString('en-US', { weekday: 'short', month: 'short', day: 'numeric', year: 'numeric' });
}

function showStatus(sm: StateManager): void {
  const plan = sm.getPlan();
  const dest = sm.getActiveDestination();
  const dates = sm.getDateAnchor();
  const dirty = sm.getDirtyFlags();

  console.log('\n╔════════════════════════════════════════════════════════════╗');
  console.log('║              TRAVEL PLAN STATUS                            ║');
  console.log('╚════════════════════════════════════════════════════════════╝\n');

  console.log(`Active Destination: ${dest}`);
  
  if (dates) {
    console.log(`Travel Dates: ${formatDate(dates.start)} → ${formatDate(dates.end)} (${dates.days} days)`);
  }

  console.log('\nProcess Status:');
  console.log('─'.repeat(50));

  const destObj = plan.destinations[dest];
  if (destObj) {
    const processes = [
      { id: 'process_1_date_anchor', name: 'P1 Date Anchor' },
      { id: 'process_2_destination', name: 'P2 Destination' },
      { id: 'process_3_4_packages', name: 'P3+4 Packages' },
      { id: 'process_3_transportation', name: 'P3 Transport' },
      { id: 'process_4_accommodation', name: 'P4 Accommodation' },
      { id: 'process_5_itinerary', name: 'P5 Itinerary' },
    ];

    for (const p of processes) {
      const proc = destObj[p.id] as Record<string, unknown> | undefined;
      const status = proc?.status as string || 'pending';
      const isDirty = dirty.destinations[dest]?.[p.id as keyof typeof dirty.destinations[typeof dest]]?.dirty;
      
      const statusIcon = {
        pending: '⏳',
        researching: '🔍',
        researched: '📋',
        selecting: '🎯',
        selected: '✅',
        populated: '📦',
        booking: '💳',
        booked: '🎫',
        confirmed: '✓',
        skipped: '⏭️',
      }[status] || '❓';

      const dirtyFlag = isDirty ? ' ⚠️ DIRTY' : '';
      console.log(`  ${statusIcon} ${p.name.padEnd(20)} ${status}${dirtyFlag}`);
    }
  }

  // Show chosen offer if any
  const packages = destObj?.process_3_4_packages as Record<string, unknown> | undefined;
  const chosenOffer = packages?.chosen_offer as Record<string, unknown> | undefined;
  if (chosenOffer) {
    console.log('\nSelected Offer:');
    console.log('─'.repeat(50));
    console.log(`  ID: ${chosenOffer.id}`);
    console.log(`  Date: ${chosenOffer.selected_date}`);
    console.log(`  Selected: ${chosenOffer.selected_at}`);
  }

  console.log('\n');
}

async function main(): Promise<void> {
  const args = process.argv.slice(2);
  const command = args[0];

  if (!command || command === 'help' || command === '--help' || command === '-h') {
    console.log(HELP);
    process.exit(0);
  }

  const dryRun = args.includes('--dry-run');
  const verbose = args.includes('--verbose');
  
  // Filter out flags from args
  const cleanArgs = args.filter(a => !a.startsWith('--'));

  const sm = new StateManager();

  try {
    switch (command) {
      case 'status': {
        showStatus(sm);
        break;
      }

      case 'set-dates': {
        const [, startDate, endDate, ...reasonParts] = cleanArgs;
        if (!startDate || !endDate) {
          console.error('Error: set-dates requires <start> and <end> dates');
          console.error('Example: set-dates 2026-02-13 2026-02-17 "Agent offered Feb 13"');
          process.exit(1);
        }

        const reason = reasonParts.join(' ') || undefined;
        
        console.log(`\n📅 Setting dates: ${formatDate(startDate)} → ${formatDate(endDate)}`);
        if (reason) console.log(`   Reason: ${reason}`);

        if (!dryRun) {
          sm.setDateAnchor(startDate, endDate, reason);
          sm.save();
          console.log('✅ Dates updated and cascade triggered');
        } else {
          console.log('🔸 DRY RUN - no changes saved');
        }

        if (verbose) showStatus(sm);
        break;
      }

      case 'update-offer': {
        const [, offerId, date, availability, priceStr, seatsStr, source] = cleanArgs;
        if (!offerId || !date || !availability) {
          console.error('Error: update-offer requires <offer-id> <date> <availability>');
          console.error('Example: update-offer besttour_TYO05MM260211AM 2026-02-13 available 27888 2 agent');
          process.exit(1);
        }

        if (!['available', 'sold_out', 'limited'].includes(availability)) {
          console.error('Error: availability must be: available | sold_out | limited');
          process.exit(1);
        }

        const price = priceStr ? parseInt(priceStr, 10) : undefined;
        const seats = seatsStr ? parseInt(seatsStr, 10) : undefined;

        console.log(`\n📝 Updating offer availability:`);
        console.log(`   Offer: ${offerId}`);
        console.log(`   Date: ${formatDate(date)}`);
        console.log(`   Availability: ${availability}`);
        if (price) console.log(`   Price: TWD ${price.toLocaleString()}`);
        if (seats !== undefined) console.log(`   Seats: ${seats}`);
        if (source) console.log(`   Source: ${source}`);

        if (!dryRun) {
          sm.updateOfferAvailability(
            offerId,
            date,
            availability as 'available' | 'sold_out' | 'limited',
            price,
            seats,
            source || 'cli'
          );
          sm.save();
          console.log('✅ Offer availability updated');
        } else {
          console.log('🔸 DRY RUN - no changes saved');
        }
        break;
      }

      case 'select-offer': {
        const [, offerId, date] = cleanArgs;
        const noPopulate = args.includes('--no-populate');

        if (!offerId || !date) {
          console.error('Error: select-offer requires <offer-id> <date>');
          console.error('Example: select-offer besttour_TYO05MM260211AM 2026-02-13');
          process.exit(1);
        }

        console.log(`\n🎯 Selecting offer:`);
        console.log(`   Offer: ${offerId}`);
        console.log(`   Date: ${formatDate(date)}`);
        console.log(`   Populate P3/P4: ${!noPopulate}`);

        if (!dryRun) {
          sm.selectOffer(offerId, date, !noPopulate);
          sm.save();
          console.log('✅ Offer selected');
          if (!noPopulate) {
            console.log('✅ P3 (transportation) and P4 (accommodation) populated from package');
          }
        } else {
          console.log('🔸 DRY RUN - no changes saved');
        }

        if (verbose) showStatus(sm);
        break;
      }

      default:
        console.error(`Unknown command: ${command}`);
        console.log(HELP);
        process.exit(1);
    }
  } catch (error) {
    console.error(`\n❌ Error: ${(error as Error).message}`);
    if (verbose) {
      console.error((error as Error).stack);
    }
    process.exit(1);
  }
}

main();
