#!/usr/bin/env npx ts-node
/**
 * Data Consistency Validator
 *
 * Validates consistency between:
 * - CLAUDE.md OTA table and Turso ota_sources
 * - CLAUDE.md completed items vs actual files on disk
 * - Skill SKILL.md file existence
 * - node_modules readiness
 * - Currency settings in Turso config
 * - Hardcoded values in scripts (pax, prices)
 * - Scraper script file existence
 *
 * Usage:
 *   npm run validate:data
 *   npx ts-node scripts/validate-data.ts
 *   npx ts-node scripts/validate-data.ts --doctor   # Full health check
 */

import * as fs from 'fs';
import * as path from 'path';
import { TursoPipelineClient } from './turso-pipeline';

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
  const fullPath = path.join(PROJECT_ROOT, filePath);
  if (!fs.existsSync(fullPath)) return null; // migrated to DB — skip silently
  try {
    const content = fs.readFileSync(fullPath, 'utf-8');
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

function rowsToObjects(response: any, idx = 0): Record<string, any>[] {
  const result = response?.results?.[idx]?.response?.result;
  if (!result?.rows || !result?.cols) return [];
  const cols = result.cols.map((c: any) => c.name);
  return result.rows.map((row: any[]) => {
    const obj: Record<string, any> = {};
    cols.forEach((name: string, i: number) => {
      obj[name] = row[i]?.value ?? null;
    });
    return obj;
  });
}

/**
 * Validate OTA sources table structure
 */
async function validateOtaSources(client: TursoPipelineClient): Promise<OtaSourcesFile | null> {
  const rows = rowsToObjects(await client.execute(
    `SELECT source_id, name, type_json, status, scraper_script, notes FROM ota_sources ORDER BY source_id`
  ));
  if (rows.length === 0) {
    addResult('ota-sources', 'error', 'Turso ota_sources has no rows. Run npm run db:migrate:turso.');
    return null;
  }
  const sources: OtaSourcesFile = { version: 'db', sources: {} };
  for (const row of rows) {
    sources.sources[row.source_id] = {
      source_id: row.source_id,
      display_name: row.name,
      display_name_en: row.name,
      types: row.type_json ? JSON.parse(row.type_json) : [],
      currency: 'TWD',
      supported: row.status === 'active',
      scraper_script: row.scraper_script || null,
      notes: row.notes || undefined,
    };
  }

  for (const [id, source] of Object.entries(sources.sources)) {
    // Check source_id matches key
    if (source.source_id !== id) {
      addResult('ota-sources', 'error', `Source ID mismatch: key "${id}" vs source_id "${source.source_id}"`, 'turso:ota_sources');
    }

    // scraper_script is DECOMMISSIONED: the Python per-source scrapers are archived
    // (archive/broken-python-scrapers/) and replaced by the Rust CDP driver
    // (rust/crates/travel-scraper) + Turso parser_rules. Do not validate file paths
    // here, and do not require a scraper_script for "supported" sources.

    // Check currency is valid
    const validCurrencies = ['TWD', 'JPY', 'USD', 'EUR'];
    if (!validCurrencies.includes(source.currency)) {
      addResult('ota-sources', 'warning', `${id}: unknown currency "${source.currency}"`, 'turso:ota_sources');
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
 * Compare CLAUDE.md OTA table with Turso ota_sources
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
      addResult('consistency', 'warning', `CLAUDE.md lists "${entry.sourceId}" but not in Turso ota_sources`, 'CLAUDE.md');
      continue;
    }

    // Check supported status
    const jsonSupported = source.supported;
    const mdSupported = entry.supported.includes('✅');
    const mdScrapeOnly = entry.supported.includes('scrape-only');

    if (jsonSupported && !mdSupported && !mdScrapeOnly) {
      addResult('consistency', 'error', `${entry.sourceId}: Turso ota_sources says supported=true but CLAUDE.md shows unsupported`, 'CLAUDE.md');
    }
    if (!jsonSupported && mdSupported && !mdScrapeOnly) {
      addResult('consistency', 'error', `${entry.sourceId}: Turso ota_sources says supported=false but CLAUDE.md shows ✅`, 'CLAUDE.md');
    }

    // scraper_script vs CLAUDE.md consistency is DECOMMISSIONED — per-source Python
    // scrapers are archived; scraping is the Rust CDP driver + parser_rules now.
  }

  // Check for sources in JSON not in CLAUDE.md
  const claudeIds = new Set(claudeEntries.map(e => e.sourceId));
  for (const id of Object.keys(sources.sources)) {
    if (!claudeIds.has(id)) {
      addResult('consistency', 'info', `${id}: in Turso ota_sources but not listed in CLAUDE.md OTA table`, 'CLAUDE.md');
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
 * Validate destination_config consistency
 */
async function validateDestinations(client: TursoPipelineClient): Promise<void> {
  const rows = rowsToObjects(await client.execute(
    `SELECT slug FROM destination_config ORDER BY slug`
  ));
  if (rows.length === 0) {
    addResult('destinations', 'error', 'Turso destination_config has no rows. Run npm run db:migrate:turso.');
    return;
  }
  // Reference data lives in normalized Turso tables (no local JSON, no ref_path file).
  // Every registered destination should have at least one area row.
  const areaCounts = rowsToObjects(await client.execute(
    `SELECT slug, COUNT(*) AS n FROM destination_areas GROUP BY slug`
  ));
  const countBySlug = new Map<string, number>();
  for (const r of areaCounts) countBySlug.set(String(r.slug), Number(r.n));
  for (const row of rows) {
    const slug = String(row.slug);
    if (!countBySlug.get(slug)) {
      addResult('destinations', 'warning',
        `${slug}: no rows in destination_areas. Run npx ts-node scripts/seed-destination-refs.ts.`,
        'turso:destination_areas');
    }
  }

  // OTA airline reference must be seeded (consumed by compare-true-cost).
  const airlineRows = rowsToObjects(await client.execute(`SELECT COUNT(*) AS n FROM airlines`));
  if (!airlineRows.length || Number(airlineRows[0].n) === 0) {
    addResult('destinations', 'error',
      'airlines table is empty. Run npx ts-node scripts/seed-ota-knowledge.ts.',
      'turso:airlines');
  }
}

/**
 * Validate holiday calendars
 */
async function validateHolidayCalendars(client: TursoPipelineClient): Promise<void> {
  const rows = rowsToObjects(await client.execute(
    `SELECT country, year, COUNT(*) AS day_count,
            SUM(CASE WHEN is_holiday = 2 AND name IS NOT NULL THEN 1 ELSE 0 END) AS named_holidays,
            MIN(source_url) AS source_url,
            MIN(fetched_at) AS fetched_at
     FROM holidays
     GROUP BY country, year
     ORDER BY country, year`
  ));
  if (rows.length === 0) {
    addResult('holidays', 'error', 'Turso holidays has no rows. Run npm run db:migrate:turso and npm run db:fetch:holidays:tw -- <DGPA CSV URL>.');
    return;
  }
  for (const row of rows) {
    const dayCount = Number(row.day_count || 0);
    const namedHolidayCount = Number(row.named_holidays || 0);
    if (dayCount < 365) {
      addResult('holidays', 'warning', `${row.country} ${row.year}: only ${dayCount} day rows in Turso`, 'turso:holidays');
    }
    if (namedHolidayCount === 0) {
      addResult('holidays', 'error', `${row.country} ${row.year}: no named holidays in Turso`, 'turso:holidays');
    }
    if (!row.source_url || !row.fetched_at) {
      addResult('holidays', 'error', `${row.country} ${row.year}: missing source_url/fetched_at provenance`, 'turso:holidays');
    }
  }
}

async function validateReferenceTables(client: TursoPipelineClient): Promise<void> {
  const response = await client.executeBatch([
    `SELECT COUNT(*) AS count FROM hotel_areas`,
    `SELECT COUNT(*) AS count FROM transport_routes`,
    `SELECT COUNT(*) AS count FROM transport_hubs`,
    `SELECT COUNT(*) AS count FROM destination_areas`,
    `SELECT COUNT(*) AS count FROM airlines`,
    `SELECT COUNT(*) AS count FROM shaping_research_artifacts`,
    `SELECT COUNT(*) AS count FROM shaping_selected_offers`,
  ]);
  const checks = [
    { label: 'hotel_areas', seed: 'scripts/backfill-local-reference-data.ts' },
    { label: 'transport_routes', seed: 'scripts/backfill-local-reference-data.ts' },
    { label: 'transport_hubs', seed: 'scripts/backfill-local-reference-data.ts' },
    { label: 'destination_areas', seed: 'scripts/seed-destination-refs.ts' },
    { label: 'airlines', seed: 'scripts/seed-ota-knowledge.ts' },
    { label: 'shaping_research_artifacts', seed: 'scripts/backfill-local-reference-data.ts' },
    { label: 'shaping_selected_offers', seed: 'scripts/backfill-local-reference-data.ts' },
  ];
  checks.forEach(({ label, seed }, idx) => {
    const row = rowsToObjects(response, idx)[0] || {};
    const count = Number(row.count || 0);
    if (count === 0) {
      addResult('turso-reference', 'error', `Turso ${label} has no rows. Run npx ts-node ${seed}.`, `turso:${label}`);
    }
  });
}

/**
 * Validate CLAUDE.md completed items: extract file paths and check they exist.
 *
 * Matches patterns like:
 *   - ✅ Some description (`src/foo/bar.ts`)
 *   - ✅ Some description (`src/foo/bar.ts`, `src/baz.ts`)
 */
function validateCompletedItems(): void {
  const claudeMd = readFile('CLAUDE.md');
  if (!claudeMd) return;

  const completedLineRe = /^- ✅ .+$/gm;
  const filePathRe = /`((?:src|scripts|data|tests)\/[^`]+\.[a-z]+)`/g;

  let match: RegExpExecArray | null;
  while ((match = completedLineRe.exec(claudeMd)) !== null) {
    const line = match[0];
    const lineNum = claudeMd.substring(0, match.index).split('\n').length;

    let pathMatch: RegExpExecArray | null;
    filePathRe.lastIndex = 0;
    while ((pathMatch = filePathRe.exec(line)) !== null) {
      const filePath = pathMatch[1];
      if (!fileExists(filePath)) {
        addResult(
          'completed-items',
          'error',
          `Completed item references missing file: ${filePath}`,
          'CLAUDE.md',
          lineNum
        );
      }
    }
  }
}

/**
 * Validate that all declared skills have SKILL.md files.
 * Parses the skill table from CLAUDE.md and checks each path.
 */
function validateSkillFiles(): void {
  const claudeMd = readFile('CLAUDE.md');
  if (!claudeMd) return;

  // Match skill table rows: | `skill-name` | `src/skills/foo/SKILL.md` | ...
  const skillPathRe = /`(src\/skills\/[^`]+\/SKILL\.md)`/g;
  let match: RegExpExecArray | null;
  const checked = new Set<string>();

  while ((match = skillPathRe.exec(claudeMd)) !== null) {
    const skillPath = match[1];
    if (checked.has(skillPath)) continue;
    checked.add(skillPath);

    if (!fileExists(skillPath)) {
      addResult('skill-files', 'error', `Skill file not found: ${skillPath}`, 'CLAUDE.md');
    }
  }

  // Also scan the src/skills directory for any SKILL.md not listed in CLAUDE.md
  const skillsDir = path.join(PROJECT_ROOT, 'src/skills');
  if (fs.existsSync(skillsDir)) {
    for (const dir of fs.readdirSync(skillsDir)) {
      const skillMd = path.join('src/skills', dir, 'SKILL.md');
      if (fileExists(skillMd) && !checked.has(skillMd)) {
        addResult('skill-files', 'warning', `Skill exists but not listed in CLAUDE.md: ${skillMd}`);
      }
    }
  }
}

/**
 * Validate node_modules readiness.
 * Checks that node_modules exists and key binaries are available.
 */
function validateDependencies(): void {
  const nodeModules = path.join(PROJECT_ROOT, 'node_modules');
  if (!fs.existsSync(nodeModules)) {
    addResult('dependencies', 'error', 'node_modules not found — run "npm install"');
    return;
  }

  // Check key binaries
  const requiredBins = ['ts-node', 'tsc', 'vitest'];
  for (const bin of requiredBins) {
    const binPath = path.join(nodeModules, '.bin', bin);
    if (!fs.existsSync(binPath)) {
      addResult('dependencies', 'error', `Missing binary: node_modules/.bin/${bin} — run "npm install"`);
    }
  }

  // Check pre-commit hook is installed
  const hookPath = path.join(PROJECT_ROOT, '.git/hooks/pre-commit');
  if (!fs.existsSync(hookPath)) {
    addResult('dependencies', 'warning', 'Pre-commit hook not installed — run "npm run hooks:install"');
  }
}

/**
 * Validate that CLI scripts referenced in package.json exist.
 */
function validateCliScripts(): void {
  const pkg = loadJson<Record<string, unknown>>('package.json');
  if (!pkg) return;

  const scripts = pkg.scripts as Record<string, string> | undefined;
  if (!scripts) return;

  // Extract ts-node source paths from script commands
  const tsNodeRe = /ts-node\s+(src\/[^\s]+\.ts|scripts\/[^\s]+\.ts)/;

  for (const [name, cmd] of Object.entries(scripts)) {
    const match = tsNodeRe.exec(cmd);
    if (match) {
      const scriptPath = match[1];
      if (!fileExists(scriptPath)) {
        addResult('cli-scripts', 'error', `npm script "${name}" references missing file: ${scriptPath}`, 'package.json');
      }
    }
  }
}

/**
 * Main validation runner
 */
async function main(): Promise<void> {
  const isDoctor = process.argv.includes('--doctor');
  const label = isDoctor ? 'Project health check (doctor)' : 'Data consistency validation';
  console.log(`Running ${label}...\n`);

  const client = new TursoPipelineClient();

  // Always run: data consistency checks
  const sources = await validateOtaSources(client);
  if (sources) {
    validateClaudeMdConsistency(sources);
  }
  validatePythonScripts();
  await validateDestinations(client);
  await validateHolidayCalendars(client);
  await validateReferenceTables(client);

  // Always run: documentation ↔ code drift checks
  validateCompletedItems();
  validateSkillFiles();
  validateCliScripts();

  // Doctor mode: also check environment readiness
  if (isDoctor) {
    validateDependencies();
  }

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

main().catch((err) => {
  console.error(err instanceof Error ? err.message : String(err));
  process.exit(1);
});
