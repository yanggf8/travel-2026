import type { CommandHandler, CliContext } from '../shared/types';
import { registerCommand } from './registry';

const addOfferCommand: CommandHandler = {
  names: ['add-offer'],
  description: 'Manually add a single FIT or group tour offer directly to stage0_tour_group_offers (no JSON file required).',
  usage: [
    'add-offer --run <run_id> --source <id> --region <code> --depart YYYY-MM-DD --return YYYY-MM-DD',
    '         --nights N --price <twd> --title "<title>" --url <url> --hotel "<name>"',
    '         [--kind fit|group_tour] [--seats N] [--baggage N] [--flight "<desc>"] [--note "<text>"]'
  ].join('\n  '),
  requiresState: false,
  async execute(ctx: CliContext): Promise<void> {
    const { args } = ctx;

    const runId = args.optionValue('--run');
    const source = args.optionValue('--source');
    const region = args.optionValue('--region');
    const depart = args.optionValue('--depart');
    const ret = args.optionValue('--return');
    const nightsStr = args.optionValue('--nights');
    const priceStr = args.optionValue('--price');
    const title = args.optionValue('--title');
    const url = args.optionValue('--url');
    const hotel = args.optionValue('--hotel');

    if (!runId || !source || !region || !depart || !ret || !nightsStr || !priceStr || !title || !url) {
      console.error('Error: Missing required flags. See usage.');
      console.error('Required: --run --source --region --depart --return --nights --price --title --url');
      process.exit(1);
    }

    const nights = parseInt(nightsStr, 10);
    const price = parseInt(priceStr, 10);
    const seats = args.optionValue('--seats') ? parseInt(args.optionValue('--seats')!, 10) : null;
    const kind = (args.optionValue('--kind') || 'fit') as 'fit' | 'group_tour';

    const offerId = `${source}-${region}-${depart.replace(/-/g, '')}-${nights}n-${Date.now().toString(36)}`;

    const now = new Date().toISOString();

    const rawNote = args.optionValue('--note') || args.optionValue('--flight') || '';
    const rawJson = JSON.stringify({
      source: 'manual_cli',
      flight: args.optionValue('--flight') || null,
      baggage_kg: args.optionValue('--baggage') ? parseInt(args.optionValue('--baggage')!) : null,
      note: rawNote || null,
      observed_at: now,
    });

    const row = {
      run_id: runId,
      offer_id: offerId,
      source_id: source,
      dest_region: region,
      depart_date: depart,
      return_date: ret,
      nights,
      price_per_person_twd: price,
      title,
      url,
      scraped_at: now,
      hotel_name: hotel || null,
      hotel_star_rating: null,
      meals_included_count: 0,
      departure_status: 'available',
      seats_available: seats,
      min_group_size: 2,
      group_size_cap: null,
      raw_json: rawJson,
      parse_warnings_json: null,
      product_kind: kind,
    };

    const svc = require('../../services/tour-group-service');

    await svc.insertTourGroupOffers([row]);

    console.log(`✅ Offer added to DB`);
    console.log(`   offer_id: ${offerId}`);
    console.log(`   ${depart} → ${ret} | ${nights}n | ${price} TWD/pax | ${hotel || 'no hotel'}`);
    console.log(`   run: ${runId}`);
  },
};

registerCommand(addOfferCommand);
