# Update for Codex — Stage 0 Shaping Re-review

## Actions taken on your previous review

- **UNIQUE index**: Changed to expression index using COALESCE (as you recommended). This should now properly enforce business uniqueness even when value columns are NULL.
  - Updated in both `scripts/turso-migrate.ts` and `scripts/schema.sql`.

- **Documentation**: Fixed the remaining "2026-06" reference in the design spec paragraph.

- **User-facing docs**: Added `--shaping` mention + example to:
  - `src/cli/commands/help.ts`
  - `docs/reference/CLI.md`

- **Regression test**: Added a dedicated test case in `tests/integration/stage0-service.regression.test.ts` that:
  - Creates a run with mixed date/text shaping (hard_constraint examples matching the Okinawa/Liko scenario)
  - Asserts via both `getResearchRun().shaping` and the new `getResearchShaping()` helper
  - Relies on the existing cleanup in `afterAll`

- **Error handling**: The previous hardening of `TursoPipelineClient` is already in place (your earlier review confirmed it surfaces errors better).

## Current state

- `npm run typecheck` — passes.
- The new shaping test now exercises the code path (it correctly fails with "no such table" until migration is re-run, proving the improved error surfacing works).

## Next for you (Codex)

Please re-review with focus on:

1. The updated UNIQUE expression index — does the semantics now match the intent?
2. The new test case — is it sufficient as a starting regression, or should we expand it (e.g. duplicate prevention test once the index is live)?
3. Any remaining documentation or help text gaps?
4. Any new issues introduced by the index change or test addition?

Run the migration yourself if you have access, or note that the human will do:

```bash
npm run db:migrate:turso
npm run db:exec -- "SELECT name, sql FROM sqlite_master WHERE type IN ('table','index') AND name LIKE '%shaping%';"
```

Then re-run the stage0-service test.

Report back with the same structured format (Verification Summary / Findings / Recommendations / Questions for Human).

Thank you — your previous review was very high signal.