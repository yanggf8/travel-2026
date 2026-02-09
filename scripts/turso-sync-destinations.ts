import fs from 'node:fs';
import path from 'node:path';
import { TursoPipelineClient } from './turso-pipeline';

function sqlEscapeText(value: string): string {
  return value.replace(/'/g, "''");
}

function sqlText(value: string | null): string {
  if (value === null) return 'NULL';
  return `'${sqlEscapeText(value)}'`;
}

async function main(): Promise<void> {
  const configPath = path.join(process.cwd(), 'data/destinations.json');
  if (!fs.existsSync(configPath)) {
    console.error(`Destinations file not found: ${configPath}`);
    process.exit(1);
  }

  const content = JSON.parse(fs.readFileSync(configPath, 'utf-8'));
  const destinations = content.destinations || {};
  
  const client = new TursoPipelineClient();
  const sqlStatements: string[] = [];

  for (const [slug, dest] of Object.entries(destinations)) {
    const d = dest as any;
    const cols = ['slug', 'display_name', 'currency', 'timezone', 'primary_airports'];
    const values = [
      sqlText(slug),
      sqlText(d.display_name),
      sqlText(d.currency || 'JPY'),
      sqlText(d.timezone || 'Asia/Tokyo'),
      sqlText(Array.isArray(d.primary_airports) ? d.primary_airports.join(',') : null),
    ];

    const updates = cols
      .filter((c) => c !== 'slug')
      .map((c) => `${c}=excluded.${c}`)
      .join(', ');

    sqlStatements.push(
      `INSERT INTO destinations (${cols.join(',')}) VALUES (${values.join(',')}) ON CONFLICT(slug) DO UPDATE SET ${updates};`
    );
  }

  if (sqlStatements.length === 0) {
    console.log('No destinations to sync.');
    return;
  }

  console.log(`Syncing ${sqlStatements.length} destinations to Turso...`);
  await client.executeMany(sqlStatements);
  console.log('✅ Destinations sync complete.');
}

main().catch((e) => {
  console.error(`Error: ${(e as Error).message}`);
  process.exit(1);
});
