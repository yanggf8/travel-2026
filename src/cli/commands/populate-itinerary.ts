import type { CommandHandler, CliContext } from '../shared/types';
import { registerCommand } from './registry';
import {
  validateDestinationRef,
  validateDestinationRefConsistency,
  type DestinationRef,
} from '../../state/destination-ref-schema';
import { allocateClustersToDays, parseAssignments, getSessionOrderForDayType, chunkEvenly } from '../shared/itinerary-helpers';
import { loadDestinationReferenceFromTurso } from '../../services/turso-service';

const populateItineraryCommand: CommandHandler = {
  names: ['populate-itinerary'],
  description: 'Populate itinerary sessions by adding activities from destination clusters (incremental; does not overwrite days).',
  usage: 'populate-itinerary --goals "<cluster1,cluster2,...>" [--pace relaxed|balanced|packed] [--assign "<cluster:day,...>"] [--dest slug] [--force]',
  async execute(ctx: CliContext): Promise<void> {
    const { sm, args, dryRun, verbose } = ctx;
    const destination = sm.resolveDestination(args.optionValue('--dest'));
    const force = args.hasFlag('--force');
    const paceOpt = args.optionValue('--pace');
    const assignOpt = args.optionValue('--assign');
    const goalsOpt = args.optionValue('--goals');
    const pace = (paceOpt || 'balanced').toLowerCase();
    if (!['relaxed', 'balanced', 'packed'].includes(pace)) {
      console.error('Error: --pace must be one of: relaxed | balanced | packed');
      process.exit(1);
    }

    if (!goalsOpt) {
      console.error('Error: populate-itinerary requires --goals "<cluster1,cluster2,...>"');
      process.exit(1);
    }

    const goals = goalsOpt
      .split(',')
      .map(s => s.trim())
      .filter(Boolean);

    if (goals.length === 0) {
      console.error('Error: --goals had no usable cluster IDs');
      process.exit(1);
    }

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

    // Load and validate destination reference with Zod
    let ref: DestinationRef;
    try {
      const rawRef = await loadDestinationReferenceFromTurso(destination);
      ref = validateDestinationRef(rawRef, `turso:destination-ref/${destination}`);

      // Check internal consistency (dangling references)
      const refWarnings = validateDestinationRefConsistency(ref, `turso:destination-ref/${destination}`);
      if (refWarnings.length > 0 && verbose) {
        console.log('\n⚠️  Destination reference consistency warnings:');
        for (const w of refWarnings.slice(0, 5)) {
          console.log(`   - ${w}`);
        }
        if (refWarnings.length > 5) {
          console.log(`   ... and ${refWarnings.length - 5} more`);
        }
      }
    } catch (error) {
      console.error(`Error: Failed to load Turso destination reference for ${destination}`);
      console.error((error as Error).message);
      process.exit(1);
    }

    const poiById = new Map(ref.pois.map((p) => [p.id, p]));
    const areaNameById = new Map(ref.areas.map((a) => [a.id, a.name]));
    const clusters = ref.clusters;

    // Plan allocation: map clusters to day numbers.
    const explicitAssignments = parseAssignments(assignOpt);
    const allocation = allocateClustersToDays(goals, days, explicitAssignments);

    const plannedAdds: Array<{ day: number; session: 'morning' | 'noon' | 'afternoon' | 'evening'; poiId: string; title: string }> = [];
    const skipped: string[] = [];

    for (const item of allocation) {
      const cluster = clusters[item.clusterId];
      if (!cluster) {
        skipped.push(`Unknown cluster: ${item.clusterId}`);
        continue;
      }

      const poiIds = cluster.pois;
      if (poiIds.length === 0) {
        skipped.push(`Cluster has no POIs: ${item.clusterId}`);
        continue;
      }

      const day = days.find(d => d.day_number === item.dayNumber);
      if (!day) continue;

      const sessionOrder = getSessionOrderForDayType(day.day_type as string);
      const sessionCount = pace === 'relaxed' ? 1 : pace === 'balanced' ? 2 : 3;
      const usedSessions = sessionOrder.slice(0, Math.min(sessionCount, sessionOrder.length));
      const perSession = chunkEvenly(poiIds, usedSessions.length);

      // Theme/focus: set if empty (or if force).
      if (!dryRun) {
        if (force || !day.theme) {
          sm.setDayTheme(destination, item.dayNumber, cluster.name);
        }
        for (const sess of usedSessions) {
          const sessObj = day[sess] as Record<string, unknown> | undefined;
          const currentFocus = sessObj?.focus as string | null | undefined;
          if (force || !currentFocus) {
            sm.setSessionFocus(destination, item.dayNumber, sess, cluster.name);
          }
        }
      }

      for (let i = 0; i < usedSessions.length; i++) {
        const session = usedSessions[i];
        for (const poiId of perSession[i]) {
          const poi = poiById.get(poiId);
          if (!poi) {
            skipped.push(`Missing POI in ref: ${poiId}`);
            continue;
          }

          const title = poi.title;
          const existing = ((day[session] as Record<string, unknown>)?.activities as Array<{ title?: string }> | undefined) ?? [];
          const hasDup = existing.some(a => (a?.title || '').toLowerCase() === title.toLowerCase());
          if (hasDup && !force) {
            continue;
          }

          plannedAdds.push({ day: item.dayNumber, session, poiId, title });

          if (!dryRun) {
            const areaId = poi.area;
            const areaName = areaNameById.get(areaId) || areaId;
            const notesParts = [
              poi.notes,
              poi.hours ? `Hours: ${poi.hours}` : null,
              poi.address ? `Address: ${poi.address}` : null,
            ].filter(Boolean) as string[];

            sm.addActivity(destination, item.dayNumber, session, {
              title,
              area: areaName,
              nearest_station: poi.nearest_station ?? undefined,
              duration_min: poi.duration_min ?? undefined,
              booking_required: poi.booking_required ?? false,
              booking_url: poi.booking_url ?? undefined,
              cost_estimate: poi.cost_estimate ?? undefined,
              tags: poi.tags ?? [],
              notes: notesParts.length ? notesParts.join(' | ') : undefined,
              priority: poi.booking_required ? 'must' : 'want',
            });
          }
        }
      }
    }

    console.log(`\n🧩 populate-itinerary (${destination})`);
    console.log(`   Pace: ${pace}`);
    console.log(`   Goals: ${goals.join(', ')}`);
    if (assignOpt) console.log(`   Assign: ${assignOpt}`);
    console.log(`   Ref: turso:destination-ref/${destination}`);

    if (skipped.length > 0) {
      console.log('\nSkipped:');
      for (const s of skipped.slice(0, 10)) console.log(`  - ${s}`);
      if (skipped.length > 10) console.log(`  ... and ${skipped.length - 10} more`);
    }

    console.log(`\nPlanned additions: ${plannedAdds.length}`);
    for (const a of plannedAdds.slice(0, 20)) {
      console.log(`  - Day ${a.day} ${a.session}: ${a.title}`);
    }
    if (plannedAdds.length > 20) console.log(`  ... and ${plannedAdds.length - 20} more`);

    if (!dryRun) {
      await sm.saveWithTracking('populate-itinerary', `${destination} ${plannedAdds.length} activities`);
      console.log('\n✅ Itinerary populated (incremental adds)');
      console.log('\nNext action: run status --full, then adjust with updateActivity/removeActivity as needed');
    } else {
      console.log('\n🔸 DRY RUN - no changes saved');
    }
  },
};

registerCommand(populateItineraryCommand);
