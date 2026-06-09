import type { CommandHandler, CliContext } from '../shared/types';
import { registerCommand } from './registry';

const chatFormatCommand: CommandHandler = {
  names: ['chat-format'],
  description: 'Output qualified candidates in clean messenger/chat format (ready to paste).',
  usage: 'chat-format --run <run_id> [--region okinawa] [--max N] [--hotel "substring"]',
  requiresState: false,
  async execute(ctx: CliContext): Promise<void> {
    const { args } = ctx;

    const runId = args.optionValue('--run');
    const region = args.optionValue('--region') || 'okinawa';
    const max = parseInt(args.optionValue('--max') || '20', 10);
    const hotelFilter = args.optionValue('--hotel');

    if (!runId) {
      console.error('Error: --run <run_id> is required');
      process.exit(1);
    }

    const svc = require('../../services/tour-group-service');

    const rows = await svc.listTourGroupOffers({
      run_id: runId,
      dest_region: region,
    });

    // Filter qualified: return <= 2026-06-27 and has real price + hotel
    let qualified = rows
      .filter((r: any) =>
        r.return_date <= '2026-06-27' &&
        r.price_per_person_twd > 10000 &&
        r.hotel_name
      );

    if (hotelFilter) {
      const needle = hotelFilter.toLowerCase();
      qualified = qualified.filter((r: any) => 
        (r.hotel_name || '').toLowerCase().includes(needle)
      );
    }

    qualified = qualified
      .sort((a: any, b: any) => a.depart_date.localeCompare(b.depart_date) || a.price_per_person_twd - b.price_per_person_twd)
      .slice(0, max);

    if (qualified.length === 0) {
      console.log('No qualified candidates found.');
      return;
    }

    for (const r of qualified) {
      const agent = r.source_id.toUpperCase();
      const hotel = r.hotel_name;
      const nights = r.nights;
      const price = r.price_per_person_twd.toLocaleString();
      const total = (r.price_per_person_twd * 2).toLocaleString();

      // Flight line — from the de-JSON'd typed columns (was JSON.parse(raw_json)).
      let flightLine = '待確認';
      if (r.raw_flight) flightLine = r.raw_flight;
      if (r.raw_flight_outbound && r.raw_flight_return) {
        flightLine = `${r.raw_flight_outbound} / ${r.raw_flight_return}`;
      }

      const baggage = '7kg + 20kg'; // default for most FIT we see

      // 美栄橋 mention check — previously scanned the whole raw_json string; now
      // scan the typed note + every annotation value (the same text, normalized).
      const annotationText = [r.raw_note ?? '', ...(r.notes ?? []).map((n: { key: string; value: string }) => n.value)].join(' ');
      const location = annotationText.includes('美栄橋') || r.hotel_name.includes('水之都')
        ? '美栄橋站附近，國際通走路方便'
        : '中央那霸';

      const seats = r.seats_available ? r.seats_available : '待確認';

      // Rating — was raw.rating from the JSON; now a notes annotation (key 'rating').
      const rating = (r.notes ?? []).find((n: { key: string; value: string }) => n.key === 'rating')?.value || '';

      const note = r.raw_note || r.title || '';

      console.log(`${agent} | ${hotel}`);
      console.log(`日期: ${r.depart_date} → ${r.return_date} (${nights}晚)`);
      console.log(`價格: $${price} /人 (2人 $${total})`);
      console.log(`航班: ${flightLine}`);
      console.log(`行李: ${baggage}`);
      console.log(`位置: ${location}`);
      console.log(`座位: ${seats}`);
      if (rating) console.log(`評價: ${rating}`);
      console.log(`重點: ${note}`);
      console.log(`連結: ${r.url}`);
      console.log(''); // blank line between entries
    }
  },
};

registerCommand(chatFormatCommand);
