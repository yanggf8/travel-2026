# Dashboard Grant-Token Management Review Prompt

## Design To Review

The Rust dashboard worker adds owner-managed viewer grant tokens for trip sharing.

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

Security model:
- Owner scope comes only from the signed GitHub OAuth session cookie.
- `?token=` grants only plan-viewer scope, never owner scope.
- Grant tokens are opaque 128-bit random hex strings.
- Viewer auth accepts only `status='active'` rows.
- State-changing routes are `POST /grants/create` and `POST /grants/deactivate`.
- Mutation routes require owner session plus HMAC CSRF derived from `SESSION_SECRET`, action, plan, token when applicable, and the session cookie.
- Worker uses `TURSO_TOKEN` for reads and `TURSO_WRITE_TOKEN` only after owner session and CSRF validation.
- Full tokens are not displayed as visible text in history; active copy buttons necessarily include the full share URL in `data-copy-url`.

Schema:
- Existing `plan_share_tokens` table remains the compatibility table.
- New columns:
  - `status TEXT NOT NULL DEFAULT 'active'`
  - `created_by TEXT`
  - `deactivated_at TEXT`
  - `deactivated_by TEXT`
- New index:
  - `idx_plan_share_tokens_plan_status_created ON plan_share_tokens(plan_id, status, created_at DESC)`

Implementation files:
- `rust/crates/travel-cli/src/db_migrate.rs`
- `workers/trip-dashboard-rs/Cargo.toml`
- `workers/trip-dashboard-rs/Cargo.lock`
- `workers/trip-dashboard-rs/src/i18n.rs`
- `workers/trip-dashboard-rs/src/render/index.rs`
- `workers/trip-dashboard-rs/src/render/share.rs`
- `workers/trip-dashboard-rs/src/router.rs`
- `workers/trip-dashboard-rs/src/styles.css`

## Claude Review Instruction

Run this from the repo root:

```bash
mkdir -p .review
git diff -- rust/crates/travel-cli/src/db_migrate.rs \
  workers/trip-dashboard-rs/Cargo.toml \
  workers/trip-dashboard-rs/Cargo.lock \
  workers/trip-dashboard-rs/src/i18n.rs \
  workers/trip-dashboard-rs/src/render/index.rs \
  workers/trip-dashboard-rs/src/render/share.rs \
  workers/trip-dashboard-rs/src/router.rs \
  workers/trip-dashboard-rs/src/styles.css \
| claude -p 'Review this Rust/Cloudflare Worker grant-token management diff.

Focus on:
- security bugs
- owner/viewer authorization separation
- CSRF correctness
- token lifecycle correctness
- Turso migration compatibility for existing and fresh databases
- Cloudflare Worker/WASM runtime issues
- SQL injection or escaping risks
- UI behavior regressions
- test gaps that should block deployment

Return findings first, ordered by severity.
For each finding include file path and line number where possible.
If there are no blocking findings, say "No blocking findings" and list residual deployment risks.
Do not rewrite the implementation.
' > .review/claude-grant-token-management-review.md
```

After Claude writes the file, ask Codex to corroborate:

```text
Read .review/claude-grant-token-management-review.md, compare it against the current diff, and corroborate which findings are valid. Fix any confirmed blockers before deploy.
```
