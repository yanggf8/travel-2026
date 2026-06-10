import type { CommandHandler, CliContext } from '../shared/types';
import { registerCommand } from './registry';
import { loadDestinationReferenceFromTurso } from '../../services/turso-service';

/**
 * Read a destination's reference data (areas / POIs / clusters / transit / tips)
 * from the normalized Turso tables. Replaces reading the old
 * references/destinations/<slug>.json files. Throws if the slug is unknown
 * (fail loud — no local-file fallback).
 */
const queryDestinationRefCommand: CommandHandler = {
  names: ['query-destination-ref', 'destination-ref'],
  description: 'Show a destination reference (areas, POIs, clusters, transit, tips) from Turso.',
  usage: 'query-destination-ref --slug <destination_slug> [--json]',
  requiresState: false,
  async execute(ctx: CliContext): Promise<void> {
    const slug = ctx.args.optionValue('--slug') ?? ctx.args.cleanArgs[0];
    if (!slug) {
      throw new Error('query-destination-ref requires --slug <destination_slug> (e.g. tokyo_2026).');
    }

    const ref = await loadDestinationReferenceFromTurso(slug);

    if (ctx.args.hasFlag('--json')) {
      console.log(JSON.stringify(ref, null, 2));
      return;
    }

    const areas = (ref.areas as any[]) ?? [];
    const pois = (ref.pois as any[]) ?? [];
    const clusters = (ref.clusters as Record<string, any>) ?? {};
    const transit = (ref.transit_estimates as Record<string, any>) ?? {};
    const interCity = (ref.inter_city_transit as Record<string, any>) ?? {};
    const tips = (ref.tips as string[]) ?? [];

    const lines: string[] = [];
    lines.push(`# ${ref.display_name ?? slug} (${slug})`);
    lines.push(`${ref.timezone ?? '?'} · ${ref.currency ?? '?'} · airports: ${(ref.primary_airports as string[] ?? []).join(', ')}`);
    lines.push('');

    lines.push(`## Areas (${areas.length})`);
    for (const a of areas) {
      lines.push(`- ${a.name} [${a.type}] — ${a.vibe}`);
      lines.push(`    stations: ${(a.stations ?? []).join(', ')} | best_for: ${(a.best_for ?? []).join(', ')}`);
    }
    lines.push('');

    lines.push(`## POIs (${pois.length})`);
    for (const p of pois) {
      const book = p.booking_required ? ` [booking required${p.booking_url ? `: ${p.booking_url}` : ''}]` : '';
      const cost = p.cost_estimate ? ` ¥${p.cost_estimate}` : ' free';
      lines.push(`- ${p.title} (${p.area}, ${p.nearest_station}) ~${p.duration_min}min${cost}${book}`);
      if (p.notes) lines.push(`    ${p.notes}`);
      if (p.hours) lines.push(`    hours: ${p.hours}`);
    }
    lines.push('');

    const clusterEntries = Object.entries(clusters);
    lines.push(`## Clusters (${clusterEntries.length})`);
    for (const [cid, c] of clusterEntries) {
      lines.push(`- ${c.name} [${cid}] (~${c.duration_min}min, best in ${c.best_area}): ${c.description}`);
      lines.push(`    POIs: ${(c.pois ?? []).join(', ')}`);
    }
    lines.push('');

    lines.push(`## Transit`);
    for (const [k, v] of Object.entries(transit)) {
      lines.push(`- ${k}: ${v.minutes}min via ${v.line}`);
    }
    if (Object.keys(interCity).length) {
      lines.push(`### Inter-city`);
      for (const [k, v] of Object.entries(interCity)) {
        lines.push(`- ${k}: ${v.minutes}min via ${v.line}${v.station_from ? ` (${v.station_from} → ${v.station_to})` : ''}`);
      }
    }
    lines.push('');

    if (tips.length) {
      lines.push(`## Tips`);
      for (const t of tips) lines.push(`- ${t}`);
    }

    console.log(lines.join('\n'));
  },
};

registerCommand(queryDestinationRefCommand);
