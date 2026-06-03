/**
 * Deprecated.
 *
 * Destination configuration is seeded and maintained in Turso by
 * scripts/turso-migrate.ts and direct DB operations. There is no local
 * destination JSON source of truth to sync from.
 */

async function main(): Promise<void> {
  throw new Error('Destination sync from local files is disabled. Use npm run db:migrate:turso or direct Turso SQL for destination_config changes.');
}

main().catch((e) => {
  console.error(`Error: ${(e as Error).message}`);
  process.exit(1);
});
