import type { CommandHandler, CliContext } from '../shared/types';
import { registerCommand } from './registry';

/**
 * Specialized helper for BestTour (喜鴻) itinerary pages.
 * Much faster than the generic add-offer when copying from their site.
 */
const addBestTourOfferCommand: CommandHandler = {
  names: ['add-besttour-offer'],
  description: 'Fast path to record BestTour FIT/group offers directly into Turso (no JSON).',
  usage: [
    'add-besttour-offer --url <besttour itinerary url> --price <twd> --hotel "<name>"',
    '                 [--depart YYYY-MM-DD] [--return YYYY-MM-DD] [--seats N] [--note "..."] [--run <run_id>]',
    '',
    'The URL is required. Product code in the URL is used to infer nights and suggest dates.',
    'Example:',
    '  add-besttour-offer --url "https://www.besttour.com.tw/itinerary/OKA04FD260612BU" \\',
    '    --price 16888 --hotel "WBF水之都那霸酒店" --seats 2 \\',
    '    --note "FD230 13:30 | Only 6/12 + 6/26 visible in June"',
  ].join('\n  '),
  requiresState: false,
  async execute(ctx: CliContext): Promise<void> {
    const { args } = ctx;

    const url = args.optionValue('--url');
    const priceStr = args.optionValue('--price');
    const hotel = args.optionValue('--hotel');

    if (!url || !priceStr || !hotel) {
      console.error('Error: --url, --price and --hotel are required for add-besttour-offer');
      process.exit(1);
    }

    const price = parseInt(priceStr, 10);
    const seats = args.optionValue('--seats') ? parseInt(args.optionValue('--seats')!, 10) : null;
    const note = args.optionValue('--note') || '';
    const runId = args.optionValue('--run') || 'stage0-20260525-093508'; // default to the current Okinawa research run

    // Parse product code from URL
    const match = url.match(/\/itinerary\/(OKA(\d{2})([A-Z]{2}))(\d{6})([A-Z0-9]+)/);
    let nights = 3;
    let suggestedDepart: string | null = null;

    if (match) {
      const daysCode = parseInt(match[2], 10); // 04 = 4 days
      nights = Math.max(1, daysCode - 1);

      const datePart = match[4]; // 260612
      if (datePart.length === 6) {
        const y = 2000 + parseInt(datePart.slice(0, 2), 10);
        const m = datePart.slice(2, 4);
        const d = datePart.slice(4, 6);
        suggestedDepart = `${y}-${m}-${d}`;
      }
    }

    const depart = args.optionValue('--depart') || suggestedDepart;
    if (!depart) {
      console.error('Error: Could not determine depart date. Please pass --depart YYYY-MM-DD');
      process.exit(1);
    }

    // If return not given, calculate from nights + depart
    let ret = args.optionValue('--return');
    if (!ret) {
      const d = new Date(depart);
      d.setDate(d.getDate() + nights);
      ret = d.toISOString().slice(0, 10);
    }

    const title = `【機加酒．沖繩自由行${nights + 1}日】${hotel}、亞洲航空、20公斤托運行李`; // reasonable default title

    // Reuse the generic adder logic by requiring it
    const generic = require('./add-offer'); // we already have the logic there, but since it's registered we can just call the insert directly

    const svc = require('../../services/tour-group-service');

    const now = new Date().toISOString();
    const offerId = `besttour-okinawa-${depart.replace(/-/g, '')}-${nights}n-${Date.now().toString(36)}`;

    const rawJson = JSON.stringify({
      source: 'besttour_manual',
      url,
      flight: 'FD230 / FD231',
      baggage_kg: 20,
      note: note || null,
      observed_at: now,
    });

    const row = {
      run_id: runId,
      offer_id: offerId,
      source_id: 'besttour',
      dest_region: 'okinawa',
      depart_date: depart,
      return_date: ret,
      nights,
      price_per_person_twd: price,
      title,
      url,
      scraped_at: now,
      hotel_name: hotel,
      hotel_star_rating: 4,
      meals_included_count: 0,
      departure_status: 'available',
      seats_available: seats,
      min_group_size: 2,
      group_size_cap: null,
      raw_json: rawJson,
      parse_warnings_json: null,
      product_kind: 'fit',
    };

    await svc.insertTourGroupOffers([row]);

    console.log(`✅ BestTour offer saved directly to Turso`);
    console.log(`   ${depart} → ${ret} (${nights}n) | ${price} TWD/pax | ${hotel}`);
    console.log(`   run: ${runId}`);
    console.log(`   url: ${url}`);
  },
};

registerCommand(addBestTourOfferCommand);
