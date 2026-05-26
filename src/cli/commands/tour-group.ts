import * as fs from 'fs';
import * as path from 'path';
import type { CommandHandler, CliContext } from '../shared/types';
import { registerCommand } from './registry';

interface ImportFileEnvelope {
  run_id: string;
  scraped_at: string;
  source_id: string;
  dest_region: string;
  nights: number;
  tour_group_offers: any[];
}

const importTourGroupCommand: CommandHandler = {
  names: ['import-tour-group-offers'],
  description: 'Import tour-group offers from a scraper output JSON file into stage0 tables.',
  usage: 'import-tour-group-offers --run <run_id> --file <path>',
  requiresState: false,
  async execute(ctx: CliContext): Promise<void> {
    const { args } = ctx;
    const runId = args.optionValue('--run');
    const file = args.optionValue('--file');
    if (!runId || !file) {
      console.error('Error: import-tour-group-offers requires --run <run_id> and --file <path>');
      process.exit(1);
    }

    const absPath = path.resolve(file);
    if (!fs.existsSync(absPath)) {
      console.error(`Error: file not found: ${absPath}`);
      process.exit(1);
    }

    let envelope: ImportFileEnvelope;
    try {
      envelope = JSON.parse(fs.readFileSync(absPath, 'utf-8'));
    } catch (err: any) {
      console.error(`Error: failed to parse ${absPath}: ${err?.message || err}`);
      process.exit(1);
    }

    if (envelope.run_id !== runId) {
      console.error(`Error: file run_id "${envelope.run_id}" does not match --run "${runId}"`);
      process.exit(1);
    }

    const svc = require('../../services/tour-group-service');
    const existing = await svc.findScrapeAttempt(
      runId, envelope.source_id, envelope.dest_region, envelope.nights
    );
    if (!existing) {
      console.error(
        `Error: no pending attempt found for (run=${runId}, source=${envelope.source_id}, ` +
        `region=${envelope.dest_region}, nights=${envelope.nights}). ` +
        `Seed it before importing.`
      );
      process.exit(1);
    }

    const offers = Array.isArray(envelope.tour_group_offers) ? envelope.tour_group_offers : [];
    const parsed: any[] = [];
    const skipped: Array<{ offer_id: string; missing: string[] }> = [];
    for (const raw of offers) {
      const v = svc.validateOfferRow(raw);
      if (v.ok) {
        parsed.push(raw);
      } else {
        skipped.push({ offer_id: raw.offer_id ?? '<no-id>', missing: v.missing });
      }
    }

    if (parsed.length > 0) {
      await svc.insertTourGroupOffers(parsed);
    }

    const status =
      parsed.length === 0 ? 'failed'
        : skipped.length > 0 ? 'partial'
          : 'ok';

    await svc.upsertScrapeAttempt({
      run_id: runId,
      source_id: envelope.source_id,
      dest_region: envelope.dest_region,
      nights: envelope.nights,
      status,
      offer_count: offers.length,
      parsed_count: parsed.length,
      skipped_count: skipped.length,
      error: skipped.length > 0
        ? `skipped ${skipped.length}: ${JSON.stringify(skipped.slice(0, 5))}`
        : null,
      attempted_at: new Date().toISOString(),
    });

    console.log(`✅ Imported ${parsed.length} offers (skipped ${skipped.length}). Attempt status: ${status}`);
  },
};

const queryTourGroupCommand: CommandHandler = {
  names: ['query-tour-group-offers'],
  description: 'List tour-group offers collected for a Stage 0 run.',
  usage: 'query-tour-group-offers --run <run_id> [--source <id>] [--dest-region <region>] [--nights N] [--max-price TWD] [--json]',
  requiresState: false,
  async execute(ctx: CliContext): Promise<void> {
    const { args } = ctx;
    const runId = args.optionValue('--run');
    if (!runId) {
      console.error('Error: query-tour-group-offers requires --run <run_id>');
      process.exit(1);
    }
    const svc = require('../../services/tour-group-service');
    const nightsStr = args.optionValue('--nights');
    const maxPriceStr = args.optionValue('--max-price');
    const rows = await svc.listTourGroupOffers({
      run_id: runId,
      source_id: args.optionValue('--source') || undefined,
      dest_region: args.optionValue('--dest-region') || undefined,
      nights: nightsStr ? parseInt(nightsStr, 10) : undefined,
      max_price: maxPriceStr ? parseInt(maxPriceStr, 10) : undefined,
    });
    if (args.hasFlag('--json')) {
      console.log(JSON.stringify(rows, null, 2));
      return;
    }
    if (rows.length === 0) { console.log('(no rows)'); return; }
    console.log('PRICE      SOURCE     REGION    DEPART      NIGHTS  HOTEL                          STAR  TITLE');
    console.log('-'.repeat(120));
    for (const r of rows) {
      const price = String(r.price_per_person_twd).padStart(8, ' ');
      const src = (r.source_id || '').padEnd(10);
      const reg = (r.dest_region || '').padEnd(9);
      const dt = (r.depart_date || '').padEnd(11);
      const n = String(r.nights).padStart(6);
      const h = (r.hotel_name || '').slice(0, 30).padEnd(30);
      const s = (r.hotel_star_rating === null || r.hotel_star_rating === undefined)
        ? '   -'
        : `   ${r.hotel_star_rating}`;
      const title = (r.title || '').slice(0, 30);
      console.log(`${price}  ${src} ${reg} ${dt} ${n}  ${h}  ${s}  ${title}`);
    }
  },
};

interface BaselineRow {
  dest_region: string;
  depart_date: string;
  return_date: string | null;
  nights: number;
  cheapest_group_tour_twd: number | null;
  cheapest_group_tour_source: string | null;
  cheapest_group_tour_confidence: string | null;
  group_tour_count: number;
  cheapest_fit_twd: number | null;
  cheapest_fit_source: string | null;
  cheapest_fit_confidence: string | null;
  fit_count: number;
  fit_savings_vs_group_tour_twd: number | null;
  fit_savings_pct: number | null;
}

const stage0BaselineCommand: CommandHandler = {
  names: ['stage0-baseline'],
  description: 'Show the methodology baseline view (group-tour ceiling vs FIT comparable) for a Stage 0 run.',
  usage: 'stage0-baseline --run <run_id> [--dest-region <region>] [--nights N] [--json]',
  requiresState: false,
  async execute(ctx: CliContext): Promise<void> {
    const { args } = ctx;
    const runId = args.optionValue('--run');
    if (!runId) {
      console.error('Error: stage0-baseline requires --run <run_id>');
      process.exit(1);
    }

    const { getResearchRun } = require('../../services/stage0-service');
    const run = await getResearchRun(runId);
    if (!run) {
      console.error(`Error: research run not found: ${runId}`);
      process.exit(1);
    }

    const svc = require('../../services/tour-group-service');
    const nightsStr = args.optionValue('--nights');
    const filterDestRegion = args.optionValue('--dest-region') || undefined;
    const filterNights = nightsStr ? parseInt(nightsStr, 10) : undefined;

    const rows = await svc.listTourGroupOffers({
      run_id: runId,
      dest_region: filterDestRegion,
      nights: filterNights,
    });

    // Group by (dest_region, depart_date, nights) — methodology compares per-date.
    const groups = new Map<string, any[]>();
    for (const r of rows) {
      const key = `${r.dest_region}|${r.depart_date}|${r.nights}`;
      if (!groups.has(key)) groups.set(key, []);
      groups.get(key)!.push(r);
    }

    const extractConfidence = (raw: string | null | undefined): string | null => {
      if (!raw) return null;
      try {
        const parsed = JSON.parse(raw);
        return parsed.confidence ?? null;
      } catch {
        return null;
      }
    };

    const baselineRows: BaselineRow[] = [];
    for (const [, groupRows] of groups) {
      const groupTours = groupRows.filter(r => r.product_kind === 'group_tour');
      const fits = groupRows.filter(r => r.product_kind === 'fit');
      groupTours.sort((a, b) => Number(a.price_per_person_twd) - Number(b.price_per_person_twd));
      fits.sort((a, b) => Number(a.price_per_person_twd) - Number(b.price_per_person_twd));

      const cheapestGroup = groupTours[0] || null;
      const cheapestFit = fits[0] || null;
      const gtPrice = cheapestGroup ? Number(cheapestGroup.price_per_person_twd) : null;
      const fitPrice = cheapestFit ? Number(cheapestFit.price_per_person_twd) : null;
      const savings = (gtPrice !== null && fitPrice !== null) ? gtPrice - fitPrice : null;
      const pct = (savings !== null && gtPrice) ? Math.round((savings / gtPrice) * 100) : null;

      const first = groupRows[0];
      baselineRows.push({
        dest_region: first.dest_region,
        depart_date: first.depart_date,
        return_date: first.return_date,
        nights: Number(first.nights),
        cheapest_group_tour_twd: gtPrice,
        cheapest_group_tour_source: cheapestGroup?.source_id ?? null,
        cheapest_group_tour_confidence: extractConfidence(cheapestGroup?.raw_json),
        group_tour_count: groupTours.length,
        cheapest_fit_twd: fitPrice,
        cheapest_fit_source: cheapestFit?.source_id ?? null,
        cheapest_fit_confidence: extractConfidence(cheapestFit?.raw_json),
        fit_count: fits.length,
        fit_savings_vs_group_tour_twd: savings,
        fit_savings_pct: pct,
      });
    }

    // Sort: dest_region, then depart_date, then nights.
    baselineRows.sort((a, b) => {
      if (a.dest_region !== b.dest_region) return a.dest_region.localeCompare(b.dest_region);
      if (a.depart_date !== b.depart_date) return a.depart_date.localeCompare(b.depart_date);
      return a.nights - b.nights;
    });

    if (args.hasFlag('--json')) {
      console.log(JSON.stringify({
        run_id: run.run_id,
        origin_code: run.origin_code,
        window_start: run.window_start,
        window_end: run.window_end,
        pax: run.pax,
        filter: { dest_region: filterDestRegion ?? null, nights: filterNights ?? null },
        baseline_rows: baselineRows,
      }, null, 2));
      return;
    }

    console.log(`\nStage 0 Baseline — ${run.run_id}  (${run.origin_code}, ` +
      `${run.pax} pax, window ${run.window_start}..${run.window_end})`);
    const filterDesc = [
      filterDestRegion ? `region=${filterDestRegion}` : null,
      filterNights !== undefined ? `nights=${filterNights}` : null,
    ].filter(Boolean).join(', ');
    if (filterDesc) console.log(`Filter: ${filterDesc}`);
    console.log('');

    if (baselineRows.length === 0) {
      console.log('(no tour-group offers in this run — scrape or import some first)\n');
      return;
    }

    // Header
    console.log(
      'REGION    DEPART       N  GROUP_TOUR (cheapest)         FIT (cheapest)                SAVINGS'
    );
    console.log('─'.repeat(105));

    const fmtPrice = (p: number | null) => p === null ? '       -' : p.toLocaleString().padStart(8);
    const fmtCount = (n: number) => n === 0 ? ' ' : `×${n}`;
    const fmtSrc = (s: string | null, conf: string | null) => {
      if (!s) return '';
      const c = conf ? ` [${conf}]` : '';
      return `${s}${c}`;
    };

    for (const row of baselineRows) {
      const gtCell = row.cheapest_group_tour_twd !== null
        ? `${fmtPrice(row.cheapest_group_tour_twd)} ${fmtCount(row.group_tour_count).padEnd(3)} ${fmtSrc(row.cheapest_group_tour_source, row.cheapest_group_tour_confidence).slice(0, 16).padEnd(16)}`
        : `       -                          `;
      const fitCell = row.cheapest_fit_twd !== null
        ? `${fmtPrice(row.cheapest_fit_twd)} ${fmtCount(row.fit_count).padEnd(3)} ${fmtSrc(row.cheapest_fit_source, row.cheapest_fit_confidence).slice(0, 16).padEnd(16)}`
        : `       -                          `;
      const savingsCell = row.fit_savings_vs_group_tour_twd !== null
        ? `cheaper by ${row.fit_savings_vs_group_tour_twd.toLocaleString()} (${row.fit_savings_pct}%)`
        : '';
      console.log(
        `${row.dest_region.padEnd(9)} ${row.depart_date.padEnd(12)} ${String(row.nights) + 'n'} ` +
        `${gtCell}  ${fitCell}  ${savingsCell}`
      );
    }
    console.log('');
    console.log('Methodology: GROUP_TOUR = ceiling (the "I gave up shopping" upper bound). FIT = comparable; DIY must beat the FIT floor.');
    console.log('Confidence tags in [brackets] from raw_json: high = bookable verified; medium = listing-shown; low = teaser/inferred.');
    console.log('');
  },
};

registerCommand(importTourGroupCommand);
registerCommand(queryTourGroupCommand);
registerCommand(stage0BaselineCommand);
