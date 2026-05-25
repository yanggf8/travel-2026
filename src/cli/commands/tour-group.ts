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

registerCommand(importTourGroupCommand);
registerCommand(queryTourGroupCommand);
