import type { CommandHandler, CliContext } from '../shared/types';
import { registerCommand } from './registry';

/**
 * Specialized fast helper for Lifetour (五福) product/search pages.
 * Optimized for quick manual entry during Shaping Stage research.
 */
const addLifetourOfferCommand: CommandHandler = {
  names: ['add-lifetour-offer'],
  description: 'Fast path for Lifetour (五福) FIT/group offers. Writes directly to Turso (no JSON file).',
  usage: [
    'add-lifetour-offer --url <lifetour url> --price <twd> --hotel "<name>"',
    '                 [--depart YYYY-MM-DD] [--return YYYY-MM-DD] [--seats N] [--note "..."] [--run <run_id>]',
    '',
    'The URL can be either a search list page or a specific product page.',
    'Example:',
    '  add-lifetour-offer --url "https://tour.lifetour.com.tw/searchlist/tpe/0001-0005" \\',
    '    --price 17200 --hotel "水之都那霸飯店" --seats 2 --depart 2026-06-21 --return 2026-06-24',
  ].join('\n  '),
  requiresState: false,
  async execute(ctx: CliContext): Promise<void> {
    const { args } = ctx;

    const url = args.optionValue('--url');
    const priceStr = args.optionValue('--price');
    const hotel = args.optionValue('--hotel');

    if (!url || !priceStr || !hotel) {
      console.error('Error: --url, --price and --hotel are required');
      process.exit(1);
    }

    const price = parseInt(priceStr, 10);
    const seats = args.optionValue('--seats') ? parseInt(args.optionValue('--seats')!, 10) : null;
    const note = args.optionValue('--note') || '';
    const runId = args.optionValue('--run') || 'shaping-20260525-093508';

    // Try to detect Okinawa from the common Lifetour code
    let region = 'okinawa';
    if (url.includes('0001-0005')) {
      region = 'okinawa';
    } else if (url.includes('0001-0001')) {
      region = 'tokyo'; // or kanto
    } else if (url.includes('0001-0003')) {
      region = 'kansai';
    }

    const nightsStr = args.optionValue('--nights');
    let nights = nightsStr ? parseInt(nightsStr, 10) : 3;

    let depart = args.optionValue('--depart');
    let ret = args.optionValue('--return');

    // If user didn't provide dates, we still require them for now
    if (!depart || !ret) {
      console.error('Error: Please provide --depart and --return (YYYY-MM-DD)');
      console.error('Lifetour pages are usually search results, so dates are not embedded in the URL.');
      process.exit(1);
    }

    if (!nightsStr) {
      // Try to infer nights from return - depart if possible
      const d1 = new Date(depart);
      const d2 = new Date(ret);
      const diffDays = Math.round((d2.getTime() - d1.getTime()) / (1000 * 3600 * 24));
      nights = Math.max(1, diffDays - 1);
    }

    const title = `【機加酒．沖繩自由行${nights + 1}日】${hotel}、五福旅遊`;

    const now = new Date().toISOString();
    const offerId = `lifetour-okinawa-${depart.replace(/-/g, '')}-${nights}n-${Date.now().toString(36)}`;

    // De-JSON'd: typed columns for read fields, flat notes for the rest (no JSON).
    const notes: Array<{ key: string; value: string }> = [
      { key: 'source', value: 'lifetour_manual' },
      { key: 'url', value: url },
      { key: 'observed_at', value: now },
    ];

    const svc = require('../../services/tour-group-service');

    const row = {
      run_id: runId,
      offer_id: offerId,
      source_id: 'lifetour',
      dest_region: region,
      depart_date: depart,
      return_date: ret,
      nights,
      price_per_person_twd: price,
      title,
      url,
      scraped_at: now,
      hotel_name: hotel,
      hotel_star_rating: null,
      meals_included_count: 0,
      departure_status: 'available',
      seats_available: seats,
      min_group_size: 2,
      group_size_cap: null,
      raw_note: note || null,
      notes,
      product_kind: 'fit',
    };

    await svc.insertTourGroupOffers([row]);

    console.log(`✅ Lifetour offer saved directly to Turso`);
    console.log(`   ${depart} → ${ret} (${nights}n) | ${price} TWD/pax | ${hotel}`);
    console.log(`   run: ${runId}`);
    console.log(`   url: ${url}`);
  },
};

registerCommand(addLifetourOfferCommand);
