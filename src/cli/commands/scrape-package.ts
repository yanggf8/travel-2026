import type { CommandHandler, CliContext } from '../shared/types';
import { registerCommand } from './registry';
import { validatePositiveInt } from '../../types/validation';
import { inferSourceIdFromUrl, inferRegionFromDestination } from '../../config/loader';
import { normalizeScrapeToOffer } from '../shared/scrape-helpers';
import { execFileSync } from 'child_process';
import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';

const scrapePackageCommand: CommandHandler = {
  names: ['scrape-package'],
  description: 'Scrape a package itinerary URL and import it into P3+4 offers.',
  usage: 'scrape-package <url> [--pax N] [--dest slug] [--force]',
  async execute(ctx: CliContext): Promise<void> {
    const { sm, args, dryRun } = ctx;
    const [, url] = args.cleanArgs;
    if (!url) {
      console.error('Error: scrape-package requires <url>');
      console.error('Example: scrape-package "https://www.besttour.com.tw/itinerary/TYO05MM260211AM" --pax 2');
      process.exit(1);
    }

    const destination = sm.resolveDestination(args.optionValue('--dest'));
    let pax = 2;
    const paxOpt = args.optionValue('--pax');
    if (paxOpt) {
      const paxResult = validatePositiveInt(paxOpt, '--pax');
      if (!paxResult.ok) {
        console.error(`Error: ${paxResult.error}`);
        process.exit(1);
      }
      pax = paxResult.value;
    }

    // Freshness check -- skip scraping if Turso has recent data
    if (!dryRun && !args.hasFlag('--force')) {
      try {
        const { checkFreshness } = await import('../../services/turso-service');
        const sourceId = inferSourceIdFromUrl(url);
        const region = inferRegionFromDestination(destination);
        const freshness = await checkFreshness(sourceId, { region });
        if (freshness.recommendation === 'skip') {
          console.log(`\nTurso has fresh data for ${sourceId} (${freshness.ageHours?.toFixed(1)}h old, ${freshness.offerCount} offers).`);
          console.log('Use --force to scrape anyway, or query-offers to view existing data.');
          return;
        }
        if (freshness.recommendation === 'rescrape') {
          console.log(`\nTurso data for ${sourceId} is ${freshness.ageHours?.toFixed(1)}h old. Re-scraping...`);
        }
      } catch {
        // Turso not configured -- proceed with scrape
      }
    }

    const tmpOut = path.join(os.tmpdir(), `package-scrape-${Date.now()}.json`);
    console.log(`\n🕷️  Scraping package URL: ${url}`);
    console.log(`   Destination: ${destination}`);
    console.log(`   Pax: ${pax}`);

    if (!dryRun) {
      execFileSync('python', ['scripts/scrape_package.py', '--quiet', url, tmpOut], { stdio: 'inherit' });
    } else {
      console.log('🔸 DRY RUN - scraper not executed');
    }

    const scrape = dryRun ? null : JSON.parse(fs.readFileSync(tmpOut, 'utf-8')) as any;

    const warnings: string[] = [];
    const offers: any[] = [];

    if (!dryRun) {
      const normalized = normalizeScrapeToOffer(scrape, pax, warnings);
      offers.push(normalized);
    }

    if (!dryRun) {
      sm.importPackageOffers(
        destination,
        offers[0]?.source_id || 'unknown',
        offers,
        `Imported from scrape-package CLI (${offers.length} offer)`,
        warnings
      );
      await sm.saveWithTracking('import-offers', `${offers.length} offers from ${offers[0]?.source_id || 'unknown'}`);
      console.log('✅ Imported offers into process_3_4_packages.results.offers');

      // Auto-import to Turso (file still exists)
      try {
        const { importOffersFromFiles } = await import('../../services/turso-service');
        const tursoResult = await importOffersFromFiles([tmpOut], {
          destination,
          region: inferRegionFromDestination(destination),
        });
        console.log(`  Turso: imported ${tursoResult.imported} offer(s)`);
      } catch (e) {
        console.warn(`  Turso auto-import skipped: ${(e as Error).message}`);
      }

      // Clean up temp file after both imports
      fs.unlinkSync(tmpOut);

      const best = offers[0]?.best_value?.date;
      if (best) {
        console.log(`\nNext action: npx ts-node src/cli/travel-update.ts select-offer ${offers[0].id} ${best}`);
      } else {
        console.log('\nNext action: review offers then run select-offer <offer-id> <date>');
      }
    } else {
      // Dry run -- clean up temp file
      if (fs.existsSync(tmpOut)) fs.unlinkSync(tmpOut);
    }
  },
};

registerCommand(scrapePackageCommand);
