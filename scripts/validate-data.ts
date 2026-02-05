#!/usr/bin/env npx ts-node
/**
 * Data Consistency Validator
 *
 * Validates consistency between:
 * - CLAUDE.md OTA table and data/ota-sources.json
 * - Currency settings in JSON files
 * - Hardcoded values in scripts (pax, prices)
 * - Scraper script file existence
 *
 * Usage:
 *   npm run validate:data
 *   npx ts-node scripts/validate-data.ts
 */

import * as fs from 'fs';
import * as path from 'path';

interface ValidationResult {
  category: string;
  severity: 'error' | 'warning' | 'info';
  message: string;
  file?: string;
  line?: number;
}

interface OtaSource {
  source_id: string;
  display_name: string;
  display_name_en: string;
  types: string[];
  currency: string;
  supported: boolean;
  scraper_script: string | null;
  notes?: string;
}

interface OtaSourcesFile {
  version: string;
  sources: Record<string, OtaSource>;
}

const PROJECT_ROOT = path.resolve(__dirname, '..');
const results: ValidationResult[] = [];

function addResult(
  category: string,
  severity: 'error' | 'warning' | 'info',
  message: string,
  file?: string,
  line?: number
): void {
  results.push({ category, severity, message, file, line });
}

/**
 * Load JSON file safely
 */
function loadJson<T>(filePath: string): T | null {
  try {
    const content = fs.readFileSync(path.join(PROJECT_ROOT, filePath), 'utf-8');
    return JSON.parse(content) as T;
  } catch (e) {
    addResult('file', 'error', `Failed to load ${filePath}: ${e instanceof Error ? e.message : String(e)}`);
    return null;
  }
}

/**
 * Read file content
 */
function readFile(filePath: string): string | null {
  try {
    return fs.readFileSync(path.join(PROJECT_ROOT, filePath), 'utf-8');
  } catch (e) {
    return null;
  }
}

/**
 * Check if file exists
 */
function fileExists(filePath: string): boolean {
  return fs.existsSync(path.join(PROJECT_ROOT, filePath));
}

/**
 * Validate OTA sources JSON structure
 */
function validateOtaSources(): OtaSourcesFile | null {
  const sources = loadJson<OtaSourcesFile>('data/ota-sources.json');
  if (!sources) return null;

  for (const [id, source] of Object.entries(sources.sources)) {
    // Check source_id matches key
    if (source.source_id !== id) {
      addResult('ota-sources', 'error', `Source ID mismatch: key "${id}" vs source_id "${source.source_id}"`, 'data/ota-sources.json');
    }

    // Check scraper_script exists if specified
    if (source.scraper_script && !fileExists(source.scraper_script)) {
      addResult('ota-sources', 'error', `Scraper script not found: ${source.scraper_script}`, 'data/ota-sources.json');
    }

    // Warning if supported but no scraper
    if (source.supported && !source.scraper_script) {
      addResult('ota-sources', 'warning', `${id}: marked as supported but no scraper_script`, 'data/ota-sources.json');
    }

    // Check currency is valid
    const validCurrencies = ['TWD', 'JPY', 'USD', 'EUR'];
    if (!validCurrencies.includes(source.currency)) {
      addResult('ota-sources', 'warning', `${id}: unknown currency "${source.currency}"`, 'data/ota-sources.json');
    }
  }

  return sources;
}

/**
 * Parse CLAUDE.md OTA table
 */
interface ClaudeOtaEntry {
  sourceId: string;
  name: string;
  types: string;
  supported: string;
  scraper: string;
}

function parseClaudeOtaTable(content: string): ClaudeOtaEntry[] {
  const entries: ClaudeOtaEntry[] = [];

  // Find OTA Sources table
  const tableMatch = content.match(/\| Source ID \| Name \| Type \| Supported \| Scraper \|[\s\S]*?(?=\n\n|\n###|\n##|$)/);
  if (!tableMatch) {
    addResult('claude-md', 'warning', 'Could not find OTA Sources table in CLAUDE.md', 'CLAUDE.md');
    return entries;
  }

  const lines = tableMatch[0].split('\n').filter(line => line.trim().startsWith('|'));
  // Skip header and separator
  for (let i = 2; i < lines.length; i++) {
    const cells = lines[i].split('|').map(c => c.trim()).filter(c => c);
    if (cells.length >= 5) {
      entries.push({
        sourceId: cells[0].replace(/`/g, ''),
        name: cells[1],
        types: cells[2],
        supported: cells[3],
        scraper: cells[4],
      });
    }
  }

  return entries;
}

/**
 * Compare CLAUDE.md OTA table with ota-sources.json
 */
function validateClaudeMdConsistency(sources: OtaSourcesFile): void {
  const claudeMd = readFile('CLAUDE.md');
  if (!claudeMd) {
    addResult('claude-md', 'error', 'CLAUDE.md not found', 'CLAUDE.md');
    return;
  }

  const claudeEntries = parseClaudeOtaTable(claudeMd);
  if (claudeEntries.length === 0) return;

  // Check each CLAUDE.md entry against ota-sources.json
  for (const entry of claudeEntries) {
    const source = sources.sources[entry.sourceId];

    if (!source) {
      addResult('consistency', 'warning', `CLAUDE.md lists "${entry.sourceId}" but not in ota-sources.json`, 'CLAUDE.md');
      continue;
    }

    // Check supported status
    const jsonSupported = source.supported;
    const mdSupported = entry.supported.includes('✅');
    const mdScrapeOnly = entry.supported.includes('scrape-only');

    if (jsonSupported && !mdSupported && !mdScrapeOnly) {
      addResult('consistency', 'error', `${entry.sourceId}: ota-sources.json says supported=true but CLAUDE.md shows unsupported`, 'CLAUDE.md');
    }
    if (!jsonSupported && mdSupported && !mdScrapeOnly) {
      addResult('consistency', 'error', `${entry.sourceId}: ota-sources.json says supported=false but CLAUDE.md shows ✅`, 'CLAUDE.md');
    }

    // Check scraper status
    const hasScraper = !!source.scraper_script;
    const mdHasScraper = entry.scraper.includes('✅');

    if (hasScraper && !mdHasScraper) {
      addResult('consistency', 'warning', `${entry.sourceId}: has scraper_script but CLAUDE.md shows ❌`, 'CLAUDE.md');
    }
    if (!hasScraper && mdHasScraper) {
      addResult('consistency', 'error', `${entry.sourceId}: no scraper_script but CLAUDE.md shows ✅`, 'CLAUDE.md');
    }
  }

  // Check for sources in JSON not in CLAUDE.md
  const claudeIds = new Set(claudeEntries.map(e => e.sourceId));
  for (const id of Object.keys(sources.sources)) {
    if (!claudeIds.has(id)) {
      addResult('consistency', 'info', `${id}: in ota-sources.json but not listed in CLAUDE.md OTA table`, 'CLAUDE.md');
    }
  }
}

/**
 * Check Python scripts for hardcoded values
 */
function validatePythonScripts(): void {
  const scriptsDir = path.join(PROJECT_ROOT, 'scripts');
  if (!fs.existsSync(scriptsDir)) return;

  const pythonFiles = fs.readdirSync(scriptsDir).filter(f => f.endsWith('.py'));

  for (const file of pythonFiles) {
    const content = readFile(`scripts/${file}`);
    if (!content) continue;

    const lines = content.split('\n');

    for (let i = 0; i < lines.length; i++) {
      const line = lines[i];
      const lineNum = i + 1;

      // Check for hardcoded pax (price * 2)
      if (/price\s*\*\s*2(?!\s*\.)/.test(line) && !line.includes('#')) {
        addResult('hardcoded', 'error', `Hardcoded pax: "price * 2" should use pax variable`, `scripts/${file}`, lineNum);
      }

      // Check for hardcoded exchange rates
      if (/[\d.]+\s*\*\s*3[12]\.?\d*/.test(line) || /3[12]\.?\d*\s*\*\s*[\d.]+/.test(line)) {
        if (!line.includes('#') && !line.includes('rate')) {
          addResult('hardcoded', 'warning', `Possible hardcoded exchange rate`, `scripts/${file}`, lineNum);
        }
      }

      // Check for hardcoded currencies that should be parameterized
      if (/["']TWD["']|["']USD["']|["']JPY["']/.test(line)) {
        if (!line.includes('default') && !line.includes('=') && !line.includes('#')) {
          addResult('hardcoded', 'info', `Hardcoded currency string (consider parameterizing)`, `scripts/${file}`, lineNum);
        }
      }
    }
  }
}

/**
 * Validate destinations.json consistency
 */
function validateDestinations(): void {
  const destinations = loadJson<Record<string, unknown>>('data/destinations.json');
  if (!destinations) return;

  // Check each destination
  for (const [id, dest] of Object.entries(destinations.destinations || {})) {
    const d = dest as Record<string, unknown>;

    // Check ref_path exists
    if (d.ref_path && typeof d.ref_path === 'string') {
      if (!fileExists(d.ref_path)) {
        addResult('destinations', 'error', `${id}: ref_path not found: ${d.ref_path}`, 'data/destinations.json');
      }
    }
  }
}

/**
 * Validate holiday calendars
 */
function validateHolidayCalendars(): void {
  const holidaysDir = path.join(PROJECT_ROOT, 'data/holidays');
  if (!fs.existsSync(holidaysDir)) {
    addResult('holidays', 'info', 'No holiday calendars found in data/holidays/');
    return;
  }

  const calendarFiles = fs.readdirSync(holidaysDir).filter(f => f.endsWith('.json'));

  for (const file of calendarFiles) {
    const calendar = loadJson<Record<string, unknown>>(`data/holidays/${file}`);
    if (!calendar) continue;

    // Check required fields
    if (!calendar.country) {
      addResult('holidays', 'warning', `Missing "country" field`, `data/holidays/${file}`);
    }
    if (!calendar.year) {
      addResult('holidays', 'warning', `Missing "year" field`, `data/holidays/${file}`);
    }
    if (!calendar.holidays || typeof calendar.holidays !== 'object') {
      addResult('holidays', 'error', `Missing or invalid "holidays" field`, `data/holidays/${file}`);
    }

    // Validate date formats in holidays
    if (calendar.holidays && typeof calendar.holidays === 'object') {
      for (const date of Object.keys(calendar.holidays as Record<string, unknown>)) {
        if (!/^\d{2}-\d{2}$/.test(date)) {
          addResult('holidays', 'warning', `Invalid date format "${date}" (expected MM-DD)`, `data/holidays/${file}`);
        }
      }
    }
  }
}

/**
 * Main validation runner
 */
function main(): void {
  console.log('Running data consistency validation...\n');

  // Run all validations
  const sources = validateOtaSources();
  if (sources) {
    validateClaudeMdConsistency(sources);
  }
  validatePythonScripts();
  validateDestinations();
  validateHolidayCalendars();

  // Group results by severity
  const errors = results.filter(r => r.severity === 'error');
  const warnings = results.filter(r => r.severity === 'warning');
  const infos = results.filter(r => r.severity === 'info');

  // Print results
  if (errors.length > 0) {
    console.log('## Errors\n');
    for (const r of errors) {
      const loc = r.file ? (r.line ? `${r.file}:${r.line}` : r.file) : '';
      console.log(`  ❌ [${r.category}] ${r.message}${loc ? ` (${loc})` : ''}`);
    }
    console.log('');
  }

  if (warnings.length > 0) {
    console.log('## Warnings\n');
    for (const r of warnings) {
      const loc = r.file ? (r.line ? `${r.file}:${r.line}` : r.file) : '';
      console.log(`  ⚠️  [${r.category}] ${r.message}${loc ? ` (${loc})` : ''}`);
    }
    console.log('');
  }

  if (infos.length > 0) {
    console.log('## Info\n');
    for (const r of infos) {
      const loc = r.file ? (r.line ? `${r.file}:${r.line}` : r.file) : '';
      console.log(`  ℹ️  [${r.category}] ${r.message}${loc ? ` (${loc})` : ''}`);
    }
    console.log('');
  }

  // Summary
  console.log('## Summary\n');
  console.log(`  Errors:   ${errors.length}`);
  console.log(`  Warnings: ${warnings.length}`);
  console.log(`  Info:     ${infos.length}`);

  // Exit code
  if (errors.length > 0) {
    process.exit(1);
  }
}

main();
