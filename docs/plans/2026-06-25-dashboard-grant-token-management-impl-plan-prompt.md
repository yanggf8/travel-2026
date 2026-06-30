# Claude Implementation Plan Prompt: Grant-Token Review Follow-Up

## Context

The Rust dashboard worker now implements owner-managed viewer grant tokens for shared trip links.

Corroborated review verdict:
- No blocking flaw in the owner/viewer authorization model.
- Owner scope comes only from the signed GitHub OAuth session.
- `?token=` grants only viewer scope for one plan.
- Viewer auth accepts only active grant tokens.
- Mutation routes require owner session and HMAC CSRF.
- The Worker uses `TURSO_TOKEN` for reads and `TURSO_WRITE_TOKEN` only after owner-session and CSRF validation.

One issue found during corroboration has already been fixed:
- `db_migrate.rs` now runs the grant-token `ALTER TABLE` and index block after `PHASE1_TABLES` creates `plan_share_tokens`, so fresh databases get the index too.

Live data check:
- Current `plan_share_tokens` row is 32 characters long: `dd90508f2efd063ee760197d127fffa4`.
- That matches the dashboard `is_grant_token` validator, so the legacy-token deactivation concern does not apply to the current live row.

Current verification:
- `cargo test -p travel-cli share_token` passes.
- `cargo test` in `workers/trip-dashboard-rs` passes: 140 tests.
- `cargo build --target wasm32-unknown-unknown` passes.

## Design Summary

Owner flow:
- Owner signs in through GitHub OAuth.
- Owner index `/` lists plans.
- Each plan shows either `Copy share link` for the newest active grant token or `Create grant token` if none exists.
- A `<details>` section shows grant-token history newest first.
- Active tokens can be copied or made inactive.
- Inactive tokens remain visible in history but are not accepted for viewer auth.

Recipient flow:
- Recipient opens `/?plan=<slug>&token=<grant_token>` without GitHub login.
- Recipient sees only that one plan.
- Recipient does not see owner chrome or grant-management controls.

Schema:
- Existing `plan_share_tokens` table remains the compatibility table.
- New columns:
  - `status TEXT NOT NULL DEFAULT 'active'`
  - `created_by TEXT`
  - `deactivated_at TEXT`
  - `deactivated_by TEXT`
- New index:
  - `idx_plan_share_tokens_plan_status_created ON plan_share_tokens(plan_id, status, created_at DESC)`

## Files In Scope

- `workers/trip-dashboard-rs/src/router.rs`
- `workers/trip-dashboard-rs/src/render/index.rs`
- `workers/trip-dashboard-rs/src/render/share.rs`
- `workers/trip-dashboard-rs/src/i18n.rs`
- `workers/trip-dashboard-rs/src/styles.css`
- `workers/trip-dashboard-rs/Cargo.toml`
- `workers/trip-dashboard-rs/Cargo.lock`
- `rust/crates/travel-cli/src/db_migrate.rs`

## Remaining Follow-Up To Plan

Prepare an implementation plan for the remaining pre-deploy hardening. Do not write code yet.

Plan these tasks:

1. Add unit tests for SQL safety and SQL builder shape.
   - Test `sql_quote("o'brien") == "o''brien"`.
   - Extract or otherwise test the deactivate SQL shape.
   - Assert deactivate SQL includes `status = 'active'`.
   - Assert create SQL includes `WHERE EXISTS` against `plans`.

2. Add focused tests for grant-route guard logic where practical.
   - Non-owner POST to `/grants/create` or `/grants/deactivate` returns 403.
   - Non-POST to those routes returns 405.
   - Invalid plan slug returns 400.
   - Bad CSRF returns 403.
   - Missing plan returns 404 for create.
   - Keep this plan realistic: if Worker `Request`/`Env` makes route-level unit tests too heavy, propose extracting pure helpers instead.

3. Add a small code comment around `owner_login` SQL interpolation.
   - State that `owner_login` is safe because `verify_session` returns only the allow-listed GitHub login.
   - Keep `sql_quote` as defense in depth.

4. Deployment smoke plan.
   - Run migration.
   - Set `TURSO_WRITE_TOKEN`.
   - Build Worker for release/WASM.
   - Live owner-session smoke:
     - `POST /grants/create` succeeds and redirects.
     - Newly created `?plan=<slug>&token=<token>` works for logged-out viewer.
     - `POST /grants/deactivate` marks token inactive.
     - Inactive token returns 403 for viewer.
   - This live smoke proves `getrandom` resolves to Web Crypto in Cloudflare Workers.

5. Optional UX follow-up, non-blocking.
   - Consider rendering `?grant=created` / `?grant=inactive` success text.
   - Consider replacing bare CSRF 403 with a redirect back to `/`.

## Claude Task

Write an implementation plan only. Include:
- steps in execution order,
- exact files to edit,
- tests to add,
- commands to run,
- any risk or ambiguity.

Do not implement. Do not deploy.

Write the plan to:

`.review/claude-grant-token-followup-implementation-plan.md`
