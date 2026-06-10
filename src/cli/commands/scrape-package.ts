import type { CommandHandler, CliContext } from '../shared/types';
import { registerCommand } from './registry';

// DECOMMISSIONED. The Python URL-construction scrapers it shelled
// (scripts/scrape_package.py) are archived under archive/broken-python-scrapers/
// because their template/region-code URLs 404 or land on the wrong page.
// Use the Rust CDP driver instead: drive the real page with chromeport
// (browser snapshot / fetch interact), then `parse capture <id>` → Turso.
const scrapePackageCommand: CommandHandler = {
  names: ['scrape-package'],
  description: '[DECOMMISSIONED] use the Rust CDP chromeport instead.',
  usage: 'scrape-package — DECOMMISSIONED',
  requiresState: false,
  async execute(_ctx: CliContext): Promise<void> {
    console.error(
      'scrape-package is DECOMMISSIONED. The Python URL scrapers are archived ' +
        '(archive/broken-python-scrapers/) — their constructed URLs 404 / hit wrong pages.\n' +
        'Use the Rust CDP driver:\n' +
        '  1) drive the real OTA page in Chrome via:  ./rust/target/debug/travel-scraper scrape interact <url> --source <id> --step ...\n' +
        '     (or browser snapshot on an already-open tab)\n' +
        '  2) parse + import to Turso:               ./rust/target/debug/travel-scraper parse capture <capture-id> --source <id>',
    );
    process.exit(2);
  },
};

registerCommand(scrapePackageCommand);
