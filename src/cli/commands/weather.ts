import type { CommandHandler, CliContext } from '../shared/types';
import { registerCommand } from './registry';

const fetchWeatherCommand: CommandHandler = {
  names: ['fetch-weather'],
  description: 'Fetch weather forecasts for itinerary days',
  usage: 'fetch-weather [--dest slug] [--all]',
  async execute(ctx: CliContext): Promise<void> {
    const { sm } = ctx;
    const destOpt = ctx.args.optionValue('--dest');

    if (ctx.args.hasFlag('--all')) {
      const plan = sm.getPlan();
      const destinations = Object.keys(plan.destinations || {});
      let totalUpdated = 0;
      for (const d of destinations) {
        const destObj = plan.destinations[d] as Record<string, unknown> | undefined;
        const p5 = destObj?.process_5_daily_itinerary as Record<string, unknown> | undefined;
        const days = p5?.days as Array<Record<string, unknown>> | undefined;
        if (!days || days.length === 0) { console.log(`  Skipping ${d} (no itinerary days)`); continue; }
        const firstDate = days[0].date as string;
        const lastDate = days[days.length - 1].date as string;
        const { fetchWeather } = require('../../services/weather-service');
        try {
          const forecasts = await fetchWeather(firstDate, lastDate, d);
          if (forecasts.length === 0) { console.log(`  ${d}: outside 16-day window`); continue; }
          for (let i = 0; i < days.length && i < forecasts.length; i++) {
            sm.setDayWeather(d, days[i].day_number as number, forecasts[i]);
          }
          console.log(`  ${d}: updated ${forecasts.length} day(s)`);
          totalUpdated += forecasts.length;
        } catch (e: any) {
          console.warn(`  ${d}: failed — ${e.message}`);
        }
      }
      if (totalUpdated > 0) await sm.saveWithTracking('fetch-weather', 'all destinations');
      return;
    }

    const dest = sm.resolveDestination(destOpt);
    const plan = sm.getPlan();
    const destObj = plan.destinations[dest] as Record<string, unknown> | undefined;
    if (!destObj) {
      console.error(`Destination not found: ${dest}`);
      process.exit(1);
    }

    const p5 = destObj.process_5_daily_itinerary as Record<string, unknown> | undefined;
    const days = p5?.days as Array<Record<string, unknown>> | undefined;
    if (!days || days.length === 0) {
      console.error('No itinerary days found. Run scaffold-itinerary first.');
      process.exit(1);
    }

    const firstDate = days[0].date as string;
    const lastDate = days[days.length - 1].date as string;

    const { fetchWeather } = require('../../services/weather-service');
    const forecasts = await fetchWeather(firstDate, lastDate, dest);

    if (forecasts.length === 0) {
      console.log('No forecast data available (dates may be outside 16-day window).');
      return;
    }

    for (let i = 0; i < days.length && i < forecasts.length; i++) {
      const dayNum = days[i].day_number as number;
      sm.setDayWeather(dest, dayNum, forecasts[i]);
    }

    await sm.saveWithTracking('fetch-weather', dest);

    console.log(`Weather updated for ${forecasts.length} day(s) in ${dest}:`);
    for (let i = 0; i < forecasts.length; i++) {
      const f = forecasts[i];
      const feelsLike = (f.feels_like_low_c != null && f.feels_like_high_c != null)
        ? ` (體感 ${f.feels_like_low_c}–${f.feels_like_high_c}°C)`
        : '';
      console.log(`  Day ${days[i].day_number}: ${f.weather_label} ${f.temp_low_c}–${f.temp_high_c}°C${feelsLike}, Rain: ${f.precipitation_pct}%`);
    }
  },
};

registerCommand(fetchWeatherCommand);
